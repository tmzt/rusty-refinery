use std::path::Path;

use tracing::info;

use librefinery::config::RefineryConfig;
use librefinery::events::{BeadEvent, EventStream};
use librefinery::git_ops::GitOps;
use librefinery::hooks;

/// Create a plan for a submodule:
/// 1. Create the planning directory `prds/{submodule}/` if missing
/// 2. Create an empty `PLAN.md` PRD
/// 3. Hash the PRD to get a bead ID
/// 4. Create `.beads/{bead_id}` in the actual submodule
/// 5. Register the bead in Redis
/// 6. Commit the first bead commit in the submodule
pub async fn run(submodule: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let repo_root = &config.options.repo_root;

    // Resolve the submodule
    let sub_info = config
        .options
        .submodules
        .get(submodule)
        .ok_or_else(|| format!("submodule not found: {submodule}"))?;
    let submodule_path = repo_root.join(&sub_info.path);

    if !submodule_path.exists() {
        return Err(format!(
            "submodule checkout not found at {}",
            submodule_path.display()
        )
        .into());
    }

    // 1. Create planning directory
    let prds_dir = config.options.planning_path.join("prds").join(submodule);
    std::fs::create_dir_all(&prds_dir)?;
    info!(path = %prds_dir.display(), "ensured planning directory");

    // 2. Create empty PLAN.md if it doesn't exist
    let plan_path = prds_dir.join("PLAN.md");
    if !plan_path.exists() {
        let initial_content = format!(
            "# Plan: {submodule}\n\n<!-- PRD for {submodule} — describe the work to be done -->\n"
        );
        std::fs::write(&plan_path, &initial_content)?;
        info!(path = %plan_path.display(), "created PLAN.md");
    } else {
        info!(path = %plan_path.display(), "PLAN.md already exists");
    }

    // 3. Hash the PRD to get bead ID
    let bead_id = GitOps::hash_blob(&plan_path)?;
    let prd_rel = format!("prds/{submodule}/PLAN.md");
    info!(bead_id = %&bead_id[..12], "hashed PLAN.md");

    // 4. Create .beads hierarchy in the submodule
    let beads_dir = submodule_path.join(".beads");
    std::fs::create_dir_all(&beads_dir)?;

    let bead_file = beads_dir.join(&bead_id);
    if !bead_file.exists() {
        let bead_meta = serde_json::json!({
            "bead_id": bead_id,
            "prd_path": prd_rel,
            "submodule": submodule,
            "status": "NEW",
            "created": unix_now(),
        });
        std::fs::write(&bead_file, serde_json::to_string_pretty(&bead_meta)?)?;
        info!(path = %bead_file.display(), "created bead entry");
    } else {
        info!(path = %bead_file.display(), "bead entry already exists");
    }

    // Write a .gitkeep in .beads if empty (besides our file)
    let gitkeep = beads_dir.join(".gitkeep");
    if !gitkeep.exists() {
        std::fs::write(&gitkeep, "")?;
    }

    // 5. Register in Redis
    let mut events = EventStream::connect(&config.options.redis_url).await?;

    match events.check_bead_status(&bead_id).await {
        Ok(Some(status)) => {
            info!(bead_id = %&bead_id[..12], %status, "bead already in Redis");
        }
        _ => {
            events
                .emit(BeadEvent::NewBead {
                    bead_id: bead_id.clone(),
                    prd_path: prd_rel.clone(),
                })
                .await?;
            events.set_bead_status(&bead_id, "NEW").await?;
            info!(bead_id = %&bead_id[..12], "registered bead in Redis");
        }
    }

    // 6. Commit the .beads/ hierarchy in the submodule
    let commit_result = commit_beads(&submodule_path, &bead_id).await;
    match &commit_result {
        Ok(()) => info!("committed bead entry in submodule"),
        Err(e) => info!(%e, "bead commit skipped (may already exist)"),
    }

    eprintln!("Plan created for '{submodule}':");
    eprintln!("  PRD:       {}", plan_path.display());
    eprintln!("  Bead ID:   {}", &bead_id[..12]);
    eprintln!("  Bead file: {}", bead_file.display());
    eprintln!("  Redis:     bead:status:{}", &bead_id[..12]);

    Ok(())
}

/// Stage .beads/ and commit in the submodule.
async fn commit_beads(
    submodule_path: &Path,
    bead_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // git add .beads/
    let output = tokio::process::Command::new("git")
        .args(["add", ".beads/"])
        .current_dir(submodule_path)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git add failed: {stderr}").into());
    }

    // Check if there's anything to commit
    let output = tokio::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(submodule_path)
        .output()
        .await?;
    if output.status.success() {
        // Nothing staged — already committed
        return Ok(());
    }

    // git commit
    let msg = format!("bead: register {}", &bead_id[..12]);
    let output = tokio::process::Command::new("git")
        .args(["commit", "-m", &msg])
        .current_dir(submodule_path)
        .env(hooks::SKIP_HOOK_ENV, "1")
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git commit failed: {stderr}").into());
    }

    Ok(())
}

fn unix_now() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

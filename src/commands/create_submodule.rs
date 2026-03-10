use tracing::info;

use crate::config::RefineryConfig;
use crate::git_ops;

/// List all discovered first-level submodules.
pub async fn list() -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let submodules = &config.options.submodules;

    if submodules.is_empty() {
        eprintln!("No submodules found.");
        return Ok(());
    }

    println!("{:<25} {}", "NAME", "PATH");
    println!("{:<25} {}", "----", "----");
    for info in submodules.values() {
        println!("{:<25} {}", info.name, info.path);
    }

    Ok(())
}

/// Create a new submodule:
/// 1. Init a repo at {repos_path}/{name}
/// 2. Add it as a submodule at {submodules_path}/{name}
/// 3. Set up `main` branch with an initial empty commit
/// 4. Optionally add a GitHub/remote using config (github_account)
/// 5. Create a planning PRD directory for the submodule
pub async fn run(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let repo_root = &config.options.repo_root;
    let repos_path = &config.options.repos_path;
    let submodules_path = &config.options.submodules_path;

    let bare_path = repos_path.join(name);
    let submodule_path = submodules_path.join(name);

    if submodule_path.exists() {
        return Err(format!("submodule path already exists: {}", submodule_path.display()).into());
    }

    // 1. Create the local repo
    std::fs::create_dir_all(bare_path.parent().unwrap())?;
    info!(path = %bare_path.display(), "initializing local repo");

    let output = tokio::process::Command::new("git")
        .args(["init", "--initial-branch=main"])
        .arg(&bare_path)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git init failed: {stderr}").into());
    }

    // Create an initial empty commit so the repo has a HEAD
    let output = tokio::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "Initial commit"])
        .current_dir(&bare_path)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("initial commit failed: {stderr}").into());
    }

    // 2. Optionally add remote from config
    let remote_url = if let Some(ref gh) = config.options.github_remote {
        let url = gh.url_for(name);
        let output = tokio::process::Command::new("git")
            .args(["remote", "add", &gh.remote_name, &url])
            .current_dir(&bare_path)
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("failed to add {} remote: {stderr}", gh.remote_name).into());
        }
        info!(remote = %gh.remote_name, url = %url, "added remote");
        Some((gh.remote_name.clone(), url))
    } else {
        None
    };

    // 3. Add as submodule in the parent repo (origin = local repo path)
    let origin_url = bare_path
        .canonicalize()
        .unwrap_or(bare_path.clone())
        .to_string_lossy()
        .to_string();

    // Compute the submodule path relative to repo root for git submodule add
    let submodule_rel = submodule_path
        .strip_prefix(repo_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| format!("submodules/{name}"));

    let output = tokio::process::Command::new("git")
        .args([
            "submodule",
            "add",
            "--branch",
            "main",
            &origin_url,
            &submodule_rel,
        ])
        .current_dir(repo_root)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git submodule add failed: {stderr}").into());
    }
    info!(
        name = name,
        path = %submodule_rel,
        origin = %origin_url,
        "submodule created"
    );

    // 4. Create prds directory in planning for this submodule
    let prds_dir = config.options.planning_path.join("prds").join(name);
    if !prds_dir.exists() {
        std::fs::create_dir_all(&prds_dir)?;
        std::fs::write(prds_dir.join(".gitkeep"), "")?;
        info!(path = %prds_dir.display(), "created planning PRD directory");
    }

    // Refresh submodule map
    let submodules = git_ops::discover_submodules(repo_root).unwrap_or_default();
    info!(total_submodules = submodules.len(), "submodule map refreshed");

    eprintln!("Created submodule '{name}':");
    eprintln!("  Local repo:  {}", bare_path.display());
    eprintln!("  Submodule:   {}", submodule_path.display());
    eprintln!("  Origin:      {origin_url}");
    if let Some((remote_name, url)) = &remote_url {
        eprintln!("  {remote_name}:     {url}");
    }
    eprintln!("  PRDs:        {}", prds_dir.display());

    Ok(())
}

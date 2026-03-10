use std::path::{Path, PathBuf};

use tracing::{error, info, warn};

use crate::events::{BeadEvent, EventStream};
use crate::git_ops::GitOps;

/// Environment variable set on agent child processes to prevent the
/// post-commit hook from re-syncing PRDs that the agent itself committed.
pub const SKIP_HOOK_ENV: &str = "RUSTY_REFINERY_SKIP_HOOK";

/// Generate the post-commit hook script content.
fn hook_script(refinery_bin: &str) -> String {
    format!(
        r#"#!/bin/sh
# crk post-commit hook — auto-syncs changed PRDs
[ "$RUSTY_REFINERY_SKIP_HOOK" = "1" ] && exit 0
exec "{refinery_bin}" hook post-commit
"#
    )
}

/// Resolve the git hooks directory for a repo, handling submodule `.git` files.
fn resolve_hooks_dir(repo_path: &Path) -> Result<PathBuf, String> {
    let git_path = repo_path.join(".git");
    if git_path.is_file() {
        let content = std::fs::read_to_string(&git_path)
            .map_err(|e| format!("cannot read .git file: {e}"))?;
        let gitdir = content
            .strip_prefix("gitdir: ")
            .map(|s| s.trim())
            .ok_or("invalid .git file format")?;
        let gitdir_path = if Path::new(gitdir).is_absolute() {
            PathBuf::from(gitdir)
        } else {
            repo_path.join(gitdir)
        };
        Ok(gitdir_path.join("hooks"))
    } else if git_path.is_dir() {
        Ok(git_path.join("hooks"))
    } else {
        Err(format!("no .git found at {}", repo_path.display()))
    }
}

/// Install a post-commit hook in the given git repo.
pub fn install(planning_path: &Path, refinery_bin: &str) -> Result<String, String> {
    let hooks_dir = resolve_hooks_dir(planning_path)?;

    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("cannot create hooks dir: {e}"))?;

    let hook_path = hooks_dir.join("post-commit");

    // Check for existing hook
    if hook_path.exists() {
        let existing = std::fs::read_to_string(&hook_path).unwrap_or_default();
        if existing.contains("crk") || existing.contains("rusty-refinery") {
            return Ok(format!("Hook already installed at {}", hook_path.display()));
        }
        return Err(format!(
            "Existing post-commit hook at {}. Remove it first or add crk manually.",
            hook_path.display()
        ));
    }

    let script = hook_script(refinery_bin);
    std::fs::write(&hook_path, &script)
        .map_err(|e| format!("cannot write hook: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot set hook permissions: {e}"))?;
    }

    Ok(format!("Installed post-commit hook at {}", hook_path.display()))
}

/// Uninstall the post-commit hook if it was installed by us.
pub fn uninstall(planning_path: &Path) -> Result<String, String> {
    let hooks_dir = resolve_hooks_dir(planning_path)?;

    let hook_path = hooks_dir.join("post-commit");
    if !hook_path.exists() {
        return Ok("No post-commit hook found.".to_string());
    }

    let content = std::fs::read_to_string(&hook_path)
        .map_err(|e| format!("cannot read hook: {e}"))?;
    if !content.contains("crk") || content.contains("rusty-refinery") {
        return Err(format!(
            "Hook at {} was not installed by crk. Not removing.",
            hook_path.display()
        ));
    }

    std::fs::remove_file(&hook_path)
        .map_err(|e| format!("cannot remove hook: {e}"))?;

    Ok(format!("Removed post-commit hook from {}", hook_path.display()))
}

/// Scan the planning directory for all PRD files (`.md` files under `prds/`).
///
/// Returns paths relative to the planning directory (e.g., `prds/rusty-genius/feature.md`).
/// Only scans first-level subdirectories under `prds/` — one level matches a submodule name.
pub fn scan_planning_dir(planning_path: &Path) -> Vec<PathBuf> {
    let prds_dir = planning_path.join("prds");
    if !prds_dir.is_dir() {
        return vec![];
    }

    let mut results = Vec::new();

    // Scan prds/ for first-level subdirectories (each maps to a submodule)
    let entries = match std::fs::read_dir(&prds_dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // This is a submodule directory — scan for .md files (non-recursive)
            if let Ok(files) = std::fs::read_dir(&path) {
                for file in files.flatten() {
                    let fp = file.path();
                    if fp.is_file() && fp.extension().is_some_and(|e| e == "md") {
                        // Return path relative to planning_path
                        if let Ok(rel) = fp.strip_prefix(planning_path) {
                            results.push(rel.to_path_buf());
                        }
                    }
                }
            }
        } else if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            // Top-level PRD (no submodule target)
            if let Ok(rel) = path.strip_prefix(planning_path) {
                results.push(rel.to_path_buf());
            }
        }
    }

    results
}

/// Handle a post-commit event: detect changed PRD files and sync each one.
///
/// Runs `git diff-tree` on HEAD to find changed files, filters for PRDs,
/// then hashes and registers each as a new bead via Redis.
pub async fn post_commit(
    planning_path: &Path,
    redis_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get changed files in the latest commit
    let output = tokio::process::Command::new("git")
        .args(["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"])
        .current_dir(planning_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git diff-tree failed: {stderr}").into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let changed_prds: Vec<&str> = stdout
        .lines()
        .filter(|f| f.starts_with("prds/") && f.ends_with(".md"))
        .collect();

    if changed_prds.is_empty() {
        info!("no PRD files changed in this commit");
        return Ok(());
    }

    info!(count = changed_prds.len(), "detected changed PRD files");
    sync_prd_files(planning_path, redis_url, &changed_prds).await
}

/// Scan the planning directory and sync all discovered PRD files.
pub async fn scan_and_sync(
    planning_path: &Path,
    redis_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let prd_paths = scan_planning_dir(planning_path);

    if prd_paths.is_empty() {
        info!("no PRD files found in {}", planning_path.display());
        return Ok(());
    }

    info!(count = prd_paths.len(), "discovered PRD files");

    let prd_strs: Vec<String> = prd_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let prd_refs: Vec<&str> = prd_strs.iter().map(|s| s.as_str()).collect();

    sync_prd_files(planning_path, redis_url, &prd_refs).await
}

/// Sync a list of PRD files (paths relative to planning_path) to Redis.
async fn sync_prd_files(
    planning_path: &Path,
    redis_url: &str,
    prd_rel_paths: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut events = EventStream::connect(redis_url).await?;

    for prd_rel in prd_rel_paths {
        let prd_abs = planning_path.join(prd_rel);

        if !prd_abs.exists() {
            warn!(path = %prd_rel, "PRD file not found, skipping");
            continue;
        }

        let bead_id = match GitOps::hash_blob(&prd_abs) {
            Ok(id) => id,
            Err(e) => {
                error!(path = %prd_rel, %e, "failed to hash PRD");
                continue;
            }
        };

        // Dedup check
        match events.check_bead_status(&bead_id).await {
            Ok(Some(status)) => {
                info!(bead_id = %&bead_id[..12], %status, path = %prd_rel, "bead already exists");
                continue;
            }
            Err(e) => {
                error!(%e, "Redis check failed, continuing");
            }
            _ => {}
        }

        // Register in Redis — store the planning-relative path (e.g., prds/rusty-genius/feature.md)
        if let Err(e) = events
            .emit(BeadEvent::NewBead {
                bead_id: bead_id.clone(),
                prd_path: prd_rel.to_string(),
            })
            .await
        {
            error!(%e, "failed to emit NEW_BEAD event");
        }

        if let Err(e) = events.set_bead_status(&bead_id, "NEW").await {
            error!(%e, "failed to set bead status");
        }

        info!(bead_id = %&bead_id[..12], path = %prd_rel, "synced PRD");
    }

    Ok(())
}

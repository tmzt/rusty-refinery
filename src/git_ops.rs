use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("worktree command failed: {0}")]
    Worktree(String),
    #[error("path not found in tree: {0}")]
    _PathNotFound(String),
}

/// A first-level submodule in the parent repo.
#[derive(Debug, Clone)]
pub struct SubmoduleInfo {
    /// The submodule name from .gitmodules (e.g., "rusty-genius")
    pub name: String,
    /// The checkout path relative to the repo root (e.g., "rusty-genius" or "backend/rusty-cog")
    pub path: String,
}

/// Map of submodule name → SubmoduleInfo, discovered from .gitmodules.
pub type SubmoduleMap = HashMap<String, SubmoduleInfo>;

/// Find the parent repo root by walking up from `start` looking for a `.git` directory
/// (not a .git file, which indicates a submodule).
/// Falls back to walking up from cwd if `start` doesn't exist.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let abs = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    // Try canonicalize, fall back to cwd if path doesn't exist
    let mut dir = abs.canonicalize()
        .or_else(|_| std::env::current_dir())
        .ok()?;
    loop {
        let git_path = dir.join(".git");
        if git_path.is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Discover first-level submodules by parsing `.gitmodules` in the given repo root.
/// Does not recurse into nested submodules.
pub fn discover_submodules(repo_root: &Path) -> Result<SubmoduleMap, GitError> {
    let gitmodules_path = repo_root.join(".gitmodules");
    if !gitmodules_path.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(&gitmodules_path)?;
    let mut map = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut current_path: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("[submodule \"") {
            // Flush previous entry
            if let (Some(name), Some(path)) = (current_name.take(), current_path.take()) {
                map.insert(name.clone(), SubmoduleInfo { name, path });
            }
            current_name = rest.strip_suffix("\"]").map(|s| s.to_string());
            current_path = None;
        } else if let Some(rest) = trimmed.strip_prefix("path = ") {
            current_path = Some(rest.to_string());
        }
    }
    // Flush last entry
    if let (Some(name), Some(path)) = (current_name.take(), current_path.take()) {
        map.insert(name.clone(), SubmoduleInfo { name, path });
    }

    Ok(map)
}

/// Resolve which submodule a PRD targets based on its subdirectory within the planning tree.
///
/// Given a PRD path like `prds/rusty-genius/feature.md`, extracts `rusty-genius`
/// and looks it up in the submodule map. Returns the submodule info and the
/// absolute path to the submodule's checkout directory.
pub fn resolve_target_submodule<'a>(
    prd_rel_path: &str,
    submodules: &'a SubmoduleMap,
    repo_root: &Path,
) -> Option<(&'a SubmoduleInfo, PathBuf)> {
    // Strip leading "prds/" if present
    let inner = prd_rel_path.strip_prefix("prds/").unwrap_or(prd_rel_path);

    // The first path component is the submodule name
    let submodule_name = inner.split('/').next()?;

    let info = submodules.get(submodule_name)?;
    let abs_path = repo_root.join(&info.path);
    Some((info, abs_path))
}

pub struct GitOps {
    _repo: git2::Repository,
}

impl GitOps {
    pub fn _open(repo_path: &Path) -> Result<Self, GitError> {
        let repo = git2::Repository::open(repo_path)?;
        Ok(GitOps { _repo: repo })
    }

    /// Compute the SHA-1 hash of a file as a git blob (equivalent to `git hash-object`).
    pub fn hash_blob(path: &Path) -> Result<String, GitError> {
        let data = std::fs::read(path)?;
        let oid = git2::Oid::hash_object(git2::ObjectType::Blob, &data)?;
        Ok(oid.to_string())
    }

    /// Read a file from the repository at a given revision.
    pub fn _read_file(&self, path: &str, rev: &str) -> Result<Vec<u8>, GitError> {
        let obj = self._repo.revparse_single(rev)?;
        let commit = obj.peel_to_commit()?;
        let tree = commit.tree()?;
        let entry = tree
            .get_path(Path::new(path))
            .map_err(|_| GitError::_PathNotFound(path.to_string()))?;
        let blob = self._repo.find_blob(entry.id())?;
        Ok(blob.content().to_vec())
    }

    /// Write a bead entry as a new commit on a detached tree.
    pub fn _write_bead_entry(&self, bead_id: &str, data: &[u8]) -> Result<(), GitError> {
        let head = self._repo.head()?;
        let parent_commit = head.peel_to_commit()?;
        let parent_tree = parent_commit.tree()?;

        // Create the blob
        let blob_oid = self._repo.blob(data)?;

        // Build the new tree with the bead entry
        let mut tree_builder = self._repo.treebuilder(Some(&parent_tree))?;
        let bead_path = format!(".beads/{bead_id}");
        tree_builder.insert(&bead_path, blob_oid, 0o100644)?;
        let new_tree_oid = tree_builder.write()?;
        let new_tree = self._repo.find_tree(new_tree_oid)?;

        // Create the commit
        let sig = self._repo.signature()?;
        self._repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("bead: register {bead_id}"),
            &new_tree,
            &[&parent_commit],
        )?;

        Ok(())
    }

    /// Create a git worktree using shell commands (git2 doesn't support worktrees well).
    pub async fn create_worktree(
        repo_path: &Path,
        name: &str,
        branch: &str,
    ) -> Result<PathBuf, GitError> {
        let worktree_path = repo_path.join(format!("wt-{name}"));
        let output = tokio::process::Command::new("git")
            .args([
                "worktree",
                "add",
                worktree_path.to_str().unwrap_or(""),
                "-b",
                &format!("bead/{name}"),
                branch,
            ])
            .current_dir(repo_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::Worktree(stderr.to_string()));
        }

        Ok(worktree_path)
    }

    /// Remove a git worktree using shell commands.
    pub async fn _remove_worktree(repo_path: &Path, name: &str) -> Result<(), GitError> {
        let worktree_name = format!("wt-{name}");
        let output = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force", &worktree_name])
            .current_dir(repo_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::Worktree(stderr.to_string()));
        }

        Ok(())
    }
}

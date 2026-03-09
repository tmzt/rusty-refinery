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
    PathNotFound(String),
}

pub struct GitOps {
    repo: git2::Repository,
}

impl GitOps {
    pub fn open(repo_path: &Path) -> Result<Self, GitError> {
        let repo = git2::Repository::open(repo_path)?;
        Ok(GitOps { repo })
    }

    /// Compute the SHA-1 hash of a file as a git blob (equivalent to `git hash-object`).
    pub fn hash_blob(path: &Path) -> Result<String, GitError> {
        let data = std::fs::read(path)?;
        let oid = git2::Oid::hash_object(git2::ObjectType::Blob, &data)?;
        Ok(oid.to_string())
    }

    /// Read a file from the repository at a given revision.
    pub fn read_file(&self, path: &str, rev: &str) -> Result<Vec<u8>, GitError> {
        let obj = self.repo.revparse_single(rev)?;
        let commit = obj.peel_to_commit()?;
        let tree = commit.tree()?;
        let entry = tree
            .get_path(Path::new(path))
            .map_err(|_| GitError::PathNotFound(path.to_string()))?;
        let blob = self.repo.find_blob(entry.id())?;
        Ok(blob.content().to_vec())
    }

    /// Write a bead entry as a new commit on a detached tree.
    pub fn write_bead_entry(&self, bead_id: &str, data: &[u8]) -> Result<(), GitError> {
        let head = self.repo.head()?;
        let parent_commit = head.peel_to_commit()?;
        let parent_tree = parent_commit.tree()?;

        // Create the blob
        let blob_oid = self.repo.blob(data)?;

        // Build the new tree with the bead entry
        let mut tree_builder = self.repo.treebuilder(Some(&parent_tree))?;
        let bead_path = format!(".beads/{bead_id}");
        tree_builder.insert(&bead_path, blob_oid, 0o100644)?;
        let new_tree_oid = tree_builder.write()?;
        let new_tree = self.repo.find_tree(new_tree_oid)?;

        // Create the commit
        let sig = self.repo.signature()?;
        self.repo.commit(
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
    pub async fn remove_worktree(repo_path: &Path, name: &str) -> Result<(), GitError> {
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

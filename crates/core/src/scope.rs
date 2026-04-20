use crate::{ContentHash, CtxError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub tenant: String,
    pub repo: RepoId,
    pub worktree: WorktreeId,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub ContentHash);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeId(pub ContentHash);

impl Scope {
    /// Construct a local scope. Both paths MUST be absolute; callers should canonicalize
    /// before calling. Returns an error if either path is relative.
    pub fn local(repo_abs: &Path, worktree_abs: &Path, branch: Option<String>) -> Result<Self> {
        if !repo_abs.is_absolute() {
            return Err(CtxError::Other(anyhow::anyhow!(
                "repo path must be absolute, got: {}",
                repo_abs.display()
            )));
        }
        if !worktree_abs.is_absolute() {
            return Err(CtxError::Other(anyhow::anyhow!(
                "worktree path must be absolute, got: {}",
                worktree_abs.display()
            )));
        }
        let repo_bytes = repo_abs.as_os_str().as_encoded_bytes();
        let wt_bytes = worktree_abs.as_os_str().as_encoded_bytes();
        Ok(Self {
            tenant: "local".to_string(),
            repo: RepoId(ContentHash::of(repo_bytes)),
            worktree: WorktreeId(ContentHash::of(wt_bytes)),
            branch,
        })
    }
}

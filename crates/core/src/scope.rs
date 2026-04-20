use crate::ContentHash;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub tenant: String, // "local" in Phase 1
    pub repo: RepoId,
    pub worktree: WorktreeId,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub ContentHash);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeId(pub ContentHash);

impl Scope {
    #[must_use]
    pub fn local(repo_abs_path: &str, worktree_abs_path: &str, branch: Option<String>) -> Self {
        Scope {
            tenant: "local".to_string(),
            repo: RepoId(ContentHash::of(repo_abs_path.as_bytes())),
            worktree: WorktreeId(ContentHash::of(worktree_abs_path.as_bytes())),
            branch,
        }
    }
}

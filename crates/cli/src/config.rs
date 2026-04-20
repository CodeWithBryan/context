use anyhow::{Context, Result};
use ctx_core::ContentHash;
use std::path::{Path, PathBuf};

/// Absolute, canonical path to a repo root. Errors if the path doesn't exist.
pub fn canonicalize_repo(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()))
}

/// `~/.ctx/repos/<blake3-of-abs-path>/`
pub fn per_repo_dir(abs_path: &Path) -> Result<PathBuf> {
    let base = dirs::home_dir()
        .context("cannot resolve home directory")?
        .join(".ctx")
        .join("repos");
    let hash = ContentHash::of(abs_path.as_os_str().as_encoded_bytes());
    Ok(base.join(hash.to_hex()))
}

/// `<per-repo-dir>/lance/`
pub fn lance_dir(abs_path: &Path) -> Result<PathBuf> {
    Ok(per_repo_dir(abs_path)?.join("lance"))
}

/// `<per-repo-dir>/refs.redb`
pub fn refs_file(abs_path: &Path) -> Result<PathBuf> {
    Ok(per_repo_dir(abs_path)?.join("refs.redb"))
}

pub fn ensure_per_repo_dirs(abs_path: &Path) -> Result<()> {
    let per_repo = per_repo_dir(abs_path)?;
    std::fs::create_dir_all(&per_repo)
        .with_context(|| format!("create {}", per_repo.display()))?;
    // lance dir created by LanceDB on first open, but make sure parent exists
    std::fs::create_dir_all(per_repo.join("lance"))?;
    Ok(())
}

use anyhow::Result;
use std::path::Path;

#[allow(clippy::unused_async)]
pub async fn run(path: &Path) -> Result<()> {
    let abs = crate::config::canonicalize_repo(path)?;
    crate::config::ensure_per_repo_dirs(&abs)?;
    println!("initialized ctx state for {}", abs.display());
    println!("  lance:  {}", crate::config::lance_dir(&abs)?.display());
    println!("  refs:   {}", crate::config::refs_file(&abs)?.display());
    Ok(())
}

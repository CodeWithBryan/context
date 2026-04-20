use anyhow::{Context, Result};
use ctx_core::traits::Embedder;
use ctx_core::Scope;
use ctx_embed::FastembedEmbedder;
use ctx_index::Pipeline;
use ctx_store::{LanceChunkStore, RedbRefStore};
use std::path::Path;
use std::sync::Arc;

pub async fn run(path: &Path) -> Result<()> {
    let abs = crate::config::canonicalize_repo(path)?;
    crate::config::ensure_per_repo_dirs(&abs)?;

    let embedder = FastembedEmbedder::new_default()
        .await
        .context("FastembedEmbedder::new_default")?;
    let dim = embedder.dim();
    let chunks = LanceChunkStore::open(crate::config::lance_dir(&abs)?, dim)
        .await
        .context("open chunk store")?;
    let refs = RedbRefStore::open(crate::config::refs_file(&abs)?)?;

    let tsserver = ctx_symbol::tsserver::TsServer::try_spawn(&abs).await?;
    let pipeline = Pipeline::new(chunks, refs, embedder).with_tsserver(tsserver.map(Arc::new));
    let scope = Scope::local(&abs, &abs, current_branch(&abs).ok()).context("construct scope")?;

    let report = pipeline.full_index(&scope, &abs).await?;
    println!(
        "indexed: files={}, skipped={}, chunks={}, embedded={}, symbols={}, errors={}",
        report.files_indexed,
        report.files_skipped,
        report.chunks_upserted,
        report.chunks_embedded,
        report.symbols_upserted,
        report.errors
    );
    Ok(())
}

pub(crate) fn current_branch(abs: &Path) -> anyhow::Result<String> {
    let repo = gix::discover(abs)?;
    let head = repo.head()?;
    match head.referent_name() {
        Some(name) => Ok(name.shorten().to_string()),
        None => Ok("HEAD".into()),
    }
}

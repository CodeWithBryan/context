use anyhow::Result;
use ctx_core::traits::Embedder;
use ctx_core::Scope;
use ctx_embed::{EmbedderOptions, FastembedEmbedder};
use ctx_query::Router;
use ctx_store::{LanceChunkStore, RedbRefStore};
use std::path::Path;
use std::sync::Arc;

pub async fn run(path: &Path) -> Result<()> {
    let abs = crate::config::canonicalize_repo(path)?;
    let embedder = FastembedEmbedder::new_with_options(EmbedderOptions {
        show_download_progress: false,
        ..Default::default()
    })
    .await?;
    let dim = embedder.dim();
    let chunks = LanceChunkStore::open(crate::config::lance_dir(&abs)?, dim).await?;
    let refs = RedbRefStore::open(crate::config::refs_file(&abs)?)?;
    let router = Router::new(Arc::new(chunks), Arc::new(refs), Arc::new(embedder));
    let scope = Scope::local(&abs, &abs, super::index::current_branch(&abs).ok())?;
    let status = router.status(&scope).await?;
    println!("chunks_total:     {}", status.chunks_total);
    println!("active_hashes:    {}", status.active_hashes);
    println!("embedding_model:  {}", status.embedding_model);
    println!("embedding_dim:    {}", status.embedding_dim);
    Ok(())
}

use anyhow::{Context, Result};
use ctx_core::traits::Embedder;
use ctx_core::Scope;
use ctx_embed::FastembedEmbedder;
use ctx_index::Pipeline;
use ctx_mcp::{CtxMcpServer, ProductionCtxMcpServer};
use ctx_query::Router;
use ctx_store::{LanceChunkStore, RedbRefStore};
use ctx_watch::Watcher;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

pub async fn run(path: &Path) -> Result<()> {
    let abs = crate::config::canonicalize_repo(path)?;
    crate::config::ensure_per_repo_dirs(&abs)?;

    // CRITICAL: use the silent embedder — stdout is the MCP transport.
    let embedder = FastembedEmbedder::new_silent()
        .await
        .context("FastembedEmbedder::new_silent")?;
    let dim = embedder.dim();
    let chunks = LanceChunkStore::open(crate::config::lance_dir(&abs)?, dim).await?;
    let refs = RedbRefStore::open(crate::config::refs_file(&abs)?)?;

    let chunks = Arc::new(chunks);
    let refs = Arc::new(refs);
    let embedder = Arc::new(embedder);

    let tsserver = ctx_symbol::tsserver::TsServer::try_spawn(&abs).await?;
    // Pipeline for the watcher side — shares the same Arc'd stores as the Router.
    let pipeline = Arc::new(
        Pipeline::new_shared(chunks.clone(), refs.clone(), embedder.clone())
            .with_tsserver(tsserver.map(Arc::new)),
    );
    let scope = Scope::local(&abs, &abs, super::index::current_branch(&abs).ok())?;

    // Spawn the watcher loop in the background.
    let mut handle = Watcher::start(&abs, Duration::from_millis(250))?;
    let pipeline_for_watch = pipeline.clone();
    let scope_for_watch = scope.clone();
    let abs_for_watch = abs.clone();
    tokio::spawn(async move {
        while let Some(batch) = handle.rx.recv().await {
            let paths: Vec<_> = batch.into_iter().collect();
            match pipeline_for_watch
                .incremental(&scope_for_watch, &abs_for_watch, &paths)
                .await
            {
                Ok(r) => info!(
                    "incremental: files={}, embedded={}, symbols={}",
                    r.files_indexed, r.chunks_embedded, r.symbols_upserted
                ),
                Err(e) => warn!("incremental index failed: {e}"),
            }
        }
    });

    // MCP server on the foreground — runs until stdin closes.
    let router = Router::new(chunks, refs, embedder);
    let server: ProductionCtxMcpServer = CtxMcpServer::new(Arc::new(router), scope);
    server.serve_stdio().await?;

    // Best-effort tsserver cleanup after the MCP client disconnects.
    if let Some(ts) = pipeline.tsserver_ref() {
        let _ = ts.shutdown().await;
    }
    Ok(())
}

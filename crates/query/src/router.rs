use ctx_core::traits::{ChunkStore, Embedder, Filter, RefStore, SymbolQuery};
use ctx_core::{Chunk, ContentHash, CtxError, Hit, Result, Scope, Symbol};
use std::sync::Arc;

/// Lightweight status snapshot returned by `Router::status`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    /// Total chunks in the underlying `ChunkStore` (not scope-filtered).
    pub chunks_total: u64,
    /// Number of active chunk hashes for the given scope (via `RefStore::active_hashes`).
    pub active_hashes: u64,
    /// Embedder identity — useful for debugging model drift.
    pub embedding_model: String,
    pub embedding_dim: usize,
}

/// Orchestrates the four underlying stores to answer MCP tool queries.
///
/// Scope isolation: `semantic_search` ALWAYS calls `RefStore::active_hashes(scope)`
/// and passes the result into `Filter.hash_allowlist` before hitting the
/// `ChunkStore`. This is the single enforcement point for scope isolation in
/// Phase 1. The `ChunkStore` itself does NOT filter by scope.
pub struct Router<C: ChunkStore, R: RefStore, E: Embedder> {
    chunks: Arc<C>,
    refs: Arc<R>,
    embedder: Arc<E>,
}

impl<C: ChunkStore, R: RefStore, E: Embedder> Router<C, R, E> {
    #[must_use]
    pub fn new(chunks: Arc<C>, refs: Arc<R>, embedder: Arc<E>) -> Self {
        Self {
            chunks,
            refs,
            embedder,
        }
    }

    #[must_use]
    pub fn chunks(&self) -> Arc<C> {
        self.chunks.clone()
    }

    #[must_use]
    pub fn refs(&self) -> Arc<R> {
        self.refs.clone()
    }

    #[must_use]
    pub fn embedder(&self) -> Arc<E> {
        self.embedder.clone()
    }

    /// Semantic search scoped to the given scope's active hashes.
    ///
    /// # Errors
    /// Propagates errors from the embedder or the underlying store.
    pub async fn semantic_search(&self, scope: &Scope, query: &str, k: usize) -> Result<Vec<Hit>> {
        let vectors = self.embedder.embed(&[query]).await?;
        let Some(v) = vectors.into_iter().next() else {
            return Err(CtxError::Embed("embedder returned no vectors".into()));
        };
        let active = self.refs.active_hashes(scope).await?;
        let filter = Filter {
            scope: Some(scope.clone()),
            hash_allowlist: Some(active),
            lang_allowlist: None,
            path_glob: None,
        };
        // TODO(reranker): Phase 2 — apply a cross-encoder reranker to the top-k
        // candidates returned here before passing results to the MCP caller.
        self.chunks.search(&v, k, &filter).await
    }

    /// Look up symbol definitions by name within the scope.
    ///
    /// # Errors
    /// Propagates errors from the underlying `RefStore`.
    pub async fn find_definition(&self, scope: &Scope, name: &str) -> Result<Vec<Symbol>> {
        self.refs
            .symbols(
                scope,
                SymbolQuery::Definition {
                    name: name.to_string(),
                },
            )
            .await
    }

    /// Look up symbol references by name within the scope.
    ///
    /// # Errors
    /// Propagates errors from the underlying `RefStore`.
    pub async fn find_references(&self, scope: &Scope, name: &str) -> Result<Vec<Symbol>> {
        self.refs
            .symbols(
                scope,
                SymbolQuery::References {
                    name: name.to_string(),
                },
            )
            .await
    }

    /// Look up callers of a function by name within the scope.
    ///
    /// # Errors
    /// Propagates errors from the underlying `RefStore`.
    pub async fn find_callers(&self, scope: &Scope, name: &str) -> Result<Vec<Symbol>> {
        self.refs
            .symbols(
                scope,
                SymbolQuery::Callers {
                    name: name.to_string(),
                },
            )
            .await
    }

    /// Retrieve a single chunk by its content hash.
    ///
    /// # Errors
    /// Propagates errors from the underlying `ChunkStore`.
    pub async fn get_chunk(&self, hash: ContentHash) -> Result<Option<Chunk>> {
        self.chunks.get(&hash).await
    }

    /// Return a lightweight status snapshot for the given scope.
    ///
    /// # Errors
    /// Propagates errors from the underlying stores.
    pub async fn status(&self, scope: &Scope) -> Result<Status> {
        let chunks_total = self.chunks.count().await?;
        let active = self.refs.active_hashes(scope).await?;
        Ok(Status {
            chunks_total,
            active_hashes: u64::try_from(active.len()).unwrap_or(u64::MAX),
            embedding_model: self.embedder.model_id().to_string(),
            embedding_dim: self.embedder.dim(),
        })
    }
}

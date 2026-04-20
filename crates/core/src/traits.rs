use crate::{Chunk, ChunkRef, ContentHash, Hit, Result, Scope, Symbol};
use async_trait::async_trait;
use std::collections::HashSet;

// reason: field names like `hash_allowlist` and `lang_allowlist` are intentionally
// descriptive and share the `_allowlist` suffix — renaming would hurt clarity.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub scope: Option<Scope>,
    pub hash_allowlist: Option<HashSet<ContentHash>>,
    pub lang_allowlist: Option<Vec<crate::Language>>,
    pub path_glob: Option<String>,
}

// async_trait is required because these traits are used behind `Box<dyn ...>` and
// `Arc<dyn ...>` (see Task 6 LanceChunkStore, Task 10 Router). Native async fn in
// traits are not dyn-compatible in Rust 1.94. Revisit once `dynosaur` or a
// stable stdlib path lands.
#[async_trait]
pub trait ChunkStore: Send + Sync {
    async fn upsert(&self, chunks: &[Chunk]) -> Result<()>;
    async fn get(&self, hash: &ContentHash) -> Result<Option<Chunk>>;
    async fn search(&self, query: &[f32], k: usize, filter: &Filter) -> Result<Vec<Hit>>;
    async fn delete(&self, hashes: &[ContentHash]) -> Result<()>;
    async fn count(&self) -> Result<u64>;
}

#[derive(Clone, Debug)]
pub enum SymbolQuery {
    Definition {
        name: String,
    },
    References {
        name: String,
    },
    Callers {
        name: String,
    },
    /// Not yet implemented in Phase 1 — `RefStore` impls must return
    /// `Err(CtxError::Unimplemented(...))` rather than an empty `Vec`.
    /// Implementation is scheduled for a later task when the ref-by-file
    /// index is added.
    ByFile {
        file: String,
    },
}

#[async_trait]
pub trait RefStore: Send + Sync {
    async fn bind(&self, scope: &Scope, refs: &[ChunkRef]) -> Result<()>;
    async fn active_hashes(&self, scope: &Scope) -> Result<HashSet<ContentHash>>;
    async fn upsert_symbols(&self, scope: &Scope, symbols: &[Symbol]) -> Result<()>;
    async fn symbols(&self, scope: &Scope, q: SymbolQuery) -> Result<Vec<Symbol>>;
    async fn record_file_hash(&self, scope: &Scope, file: &str, hash: ContentHash) -> Result<()>;
    async fn file_hash(&self, scope: &Scope, file: &str) -> Result<Option<ContentHash>>;
    /// Remove all refs and all symbols associated with `file` in `scope`.
    /// Called by the indexing pipeline before re-binding a changed file so
    /// stale entries from the previous content don't leak into queries.
    async fn clear_file_state(&self, scope: &Scope, file: &str) -> Result<()>;
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}

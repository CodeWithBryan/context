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
    Definition { name: String },
    References { name: String },
    Callers { name: String },
    ByFile { file: String },
}

#[async_trait]
pub trait RefStore: Send + Sync {
    async fn bind(&self, scope: &Scope, refs: &[ChunkRef]) -> Result<()>;
    async fn active_hashes(&self, scope: &Scope) -> Result<HashSet<ContentHash>>;
    async fn upsert_symbols(&self, scope: &Scope, symbols: &[Symbol]) -> Result<()>;
    async fn symbols(&self, scope: &Scope, q: SymbolQuery) -> Result<Vec<Symbol>>;
    async fn record_file_hash(&self, scope: &Scope, file: &str, hash: ContentHash) -> Result<()>;
    async fn file_hash(&self, scope: &Scope, file: &str) -> Result<Option<ContentHash>>;
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}

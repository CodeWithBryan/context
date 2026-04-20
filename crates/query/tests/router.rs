use async_trait::async_trait;
use ctx_core::traits::{ChunkStore, Embedder, RefStore, SymbolQuery};
use ctx_core::types::{ByteRange, LineRange};
use ctx_core::{Chunk, ChunkKind, ChunkRef, ContentHash, CtxError, Language, Result, Scope, Symbol};
use ctx_query::Router;
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

// --- Mocks ---

struct MockEmbedder;

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| {
            let mut v = vec![0.0_f32; 4];
            #[allow(clippy::cast_precision_loss)]
            {
                v[0] = (t.bytes().map(u32::from).sum::<u32>() % 256) as f32;
            }
            v
        }).collect())
    }
    fn dim(&self) -> usize { 4 }
    fn model_id(&self) -> &'static str { "mock" }
}

fn sample_chunk(n: u8, text: &str) -> Chunk {
    Chunk {
        hash: ContentHash([n; 32]),
        file: format!("f{n}.ts"),
        lang: Language::TypeScript,
        kind: ChunkKind::Function,
        name: Some(format!("fn{n}")),
        byte_range: ByteRange::new(0, text.len()),
        line_range: LineRange::new(0, 1),
        text: text.into(),
        vector: Some(vec![f32::from(n) * 0.1, 0.0, 0.0, 0.0]),
    }
}

fn scope_at(root: &Path) -> Scope {
    Scope::local(root, root, Some("main".into())).unwrap()
}

async fn make_router(
    dir: &Path,
) -> (
    Router<ctx_store::LanceChunkStore, ctx_store::RedbRefStore, MockEmbedder>,
    Scope,
) {
    let chunks = ctx_store::LanceChunkStore::open(dir.join("lance"), 4).await.unwrap();
    let refs = ctx_store::RedbRefStore::open(dir.join("refs.redb")).unwrap();
    let embed = MockEmbedder;
    let router = Router::new(Arc::new(chunks), Arc::new(refs), Arc::new(embed));
    let root = dir.join("minirepo");
    std::fs::create_dir_all(&root).unwrap();
    let scope = scope_at(&root);
    (router, scope)
}

#[tokio::test]
async fn semantic_search_applies_active_hash_filter() {
    let dir = tempdir().unwrap();
    let (router, scope) = make_router(dir.path()).await;

    // Insert 2 chunks directly into the ChunkStore
    router.chunks().upsert(&[sample_chunk(1, "alpha"), sample_chunk(2, "beta")]).await.unwrap();
    // Bind only chunk [1] as active in this scope
    router.refs().bind(&scope, &[ChunkRef {
        hash: ContentHash([1; 32]),
        file: "f1.ts".into(),
        line_range: LineRange::new(0, 1),
    }]).await.unwrap();

    let hits = router.semantic_search(&scope, "query text", 5).await.unwrap();
    // Even though the search is over all chunks, the active-hash filter should
    // restrict results to chunk [1] only.
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk.hash, ContentHash([1; 32]));
}

#[tokio::test]
async fn find_definition_returns_matching_symbols() {
    let dir = tempdir().unwrap();
    let (router, scope) = make_router(dir.path()).await;
    router.refs().upsert_symbols(&scope, &[Symbol {
        name: "greet".into(),
        kind: ChunkKind::Function,
        file: "a.ts".into(),
        line: 10,
        container: None,
    }]).await.unwrap();

    let syms = router.find_definition(&scope, "greet").await.unwrap();
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0].line, 10);
}

#[tokio::test]
async fn get_chunk_by_hash_roundtrips() {
    let dir = tempdir().unwrap();
    let (router, _scope) = make_router(dir.path()).await;
    let original = sample_chunk(42, "answer");
    router.chunks().upsert(std::slice::from_ref(&original)).await.unwrap();
    let got = router.get_chunk(ContentHash([42; 32])).await.unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().hash, original.hash);
}

#[tokio::test]
async fn status_reports_nonzero_counts() {
    let dir = tempdir().unwrap();
    let (router, scope) = make_router(dir.path()).await;
    router.chunks().upsert(&[sample_chunk(5, "x"), sample_chunk(6, "y")]).await.unwrap();
    router.refs().bind(&scope, &[ChunkRef {
        hash: ContentHash([5; 32]),
        file: "f5.ts".into(),
        line_range: LineRange::new(0, 1),
    }]).await.unwrap();

    let status = router.status(&scope).await.unwrap();
    assert_eq!(status.chunks_total, 2);
    assert_eq!(status.active_hashes, 1);
    assert_eq!(status.embedding_model, "mock");
    assert_eq!(status.embedding_dim, 4);
}

#[tokio::test]
async fn by_file_propagates_unimplemented_error_upstream() {
    // Router should not silently swallow the Unimplemented error
    // (we want callers to know ByFile isn't available yet)
    let dir = tempdir().unwrap();
    let (router, scope) = make_router(dir.path()).await;
    // find_definition / find_references / find_callers all use their specific variants,
    // not ByFile — so there's no Router method that hits this path today.
    // This test asserts that even if called with the ByFile variant at the RefStore
    // layer (via router.refs()), the error propagates.
    let err = router.refs()
        .symbols(&scope, SymbolQuery::ByFile { file: "a.ts".into() })
        .await
        .expect_err("ByFile should error");
    assert!(matches!(err, CtxError::Unimplemented(_)));
}

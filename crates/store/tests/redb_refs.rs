use ctx_core::scope::{RepoId, WorktreeId};
use ctx_core::traits::{RefStore, SymbolQuery};
use ctx_core::types::LineRange;
use ctx_core::{ChunkKind, ChunkRef, ContentHash, CtxError, Scope, Symbol};
use ctx_store::RedbRefStore;
use std::path::PathBuf;
use tempfile::tempdir;

fn sample_scope() -> Scope {
    let root = PathBuf::from("/tmp/repo-fixture");
    Scope::local(&root, &root, Some("main".into())).expect("scope")
}

fn sample_chunk_ref(h: u8) -> ChunkRef {
    ChunkRef {
        hash: ContentHash([h; 32]),
        file: format!("f{h}.ts"),
        line_range: LineRange::new(0, 10),
    }
}

#[tokio::test]
async fn bind_and_list_active_hashes() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let scope = sample_scope();
    store
        .bind(&scope, &[sample_chunk_ref(1), sample_chunk_ref(2)])
        .await
        .unwrap();
    let active = store.active_hashes(&scope).await.unwrap();
    assert!(active.contains(&ContentHash([1; 32])));
    assert!(active.contains(&ContentHash([2; 32])));
    assert_eq!(active.len(), 2);
}

#[tokio::test]
async fn upsert_and_find_symbol_definition() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let scope = sample_scope();
    store
        .upsert_symbols(
            &scope,
            &[Symbol {
                name: "greet".into(),
                kind: ChunkKind::Function,
                file: "a.ts".into(),
                line: 3,
                container: None,
            }],
        )
        .await
        .unwrap();
    let out = store
        .symbols(&scope, SymbolQuery::Definition { name: "greet".into() })
        .await
        .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "greet");
    assert_eq!(out[0].line, 3);
}

#[tokio::test]
async fn by_file_query_returns_unimplemented_error() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let scope = sample_scope();
    let err = store
        .symbols(&scope, SymbolQuery::ByFile { file: "a.ts".into() })
        .await
        .expect_err("ByFile should be unimplemented in Phase 1");
    assert!(matches!(err, CtxError::Unimplemented(_)));
}

#[tokio::test]
async fn file_hash_roundtrip() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let scope = sample_scope();
    let hash = ContentHash([7; 32]);
    store.record_file_hash(&scope, "a.ts", hash).await.unwrap();
    let got = store.file_hash(&scope, "a.ts").await.unwrap();
    assert_eq!(got, Some(hash));
    let missing = store.file_hash(&scope, "nope.ts").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn active_hashes_scoped_per_scope() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let root_a = PathBuf::from("/tmp/repo-a");
    let root_b = PathBuf::from("/tmp/repo-b");
    let scope_a = Scope::local(&root_a, &root_a, None).unwrap();
    let scope_b = Scope::local(&root_b, &root_b, None).unwrap();
    store.bind(&scope_a, &[sample_chunk_ref(10)]).await.unwrap();
    store.bind(&scope_b, &[sample_chunk_ref(20)]).await.unwrap();
    let a = store.active_hashes(&scope_a).await.unwrap();
    let b = store.active_hashes(&scope_b).await.unwrap();
    assert!(a.contains(&ContentHash([10; 32])));
    assert!(!a.contains(&ContentHash([20; 32])));
    assert!(b.contains(&ContentHash([20; 32])));
    assert!(!b.contains(&ContentHash([10; 32])));
}

#[tokio::test]
async fn scope_key_rejects_reserved_branch_literal_none() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let root = PathBuf::from("/tmp/repo-collision");
    // Build a scope whose branch is literally "_none" (which we reserve as the
    // scope-key None sentinel). `bind` must reject it rather than silently
    // colliding with a real None-branch scope.
    let bad_scope = Scope::local(&root, &root, Some("_none".into())).unwrap();
    let err = store.bind(&bad_scope, &[]).await.expect_err("reserved branch");
    assert!(matches!(err, ctx_core::CtxError::Store(_)),
        "expected CtxError::Store, got: {err:?}");
}

#[tokio::test]
async fn scope_key_rejects_tenant_with_colon() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let _root = PathBuf::from("/tmp/repo-tenant");
    // Construct a Scope with a tenant containing ':' directly (bypassing `local`).
    let bad_scope = Scope {
        tenant: "evil:tenant".into(),
        repo: RepoId(ContentHash::of(b"repo")),
        worktree: WorktreeId(ContentHash::of(b"wt")),
        branch: None,
    };
    let err = store.bind(&bad_scope, &[]).await.expect_err("reserved tenant");
    assert!(matches!(err, ctx_core::CtxError::Store(_)),
        "expected CtxError::Store, got: {err:?}");
}

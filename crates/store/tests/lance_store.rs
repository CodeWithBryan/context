use ctx_core::traits::{ChunkStore, Filter};
use ctx_core::types::{ByteRange, LineRange};
use ctx_core::{Chunk, ChunkKind, ContentHash, Language};
use ctx_store::LanceChunkStore;
use tempfile::tempdir;

fn chunk(i: u8, text: &str, vec: Vec<f32>) -> Chunk {
    Chunk {
        hash: ContentHash([i; 32]),
        file: format!("f{i}.ts"),
        lang: Language::TypeScript,
        kind: ChunkKind::Function,
        name: Some(format!("fn{i}")),
        byte_range: ByteRange::new(0, text.len()),
        line_range: LineRange::new(0, 1),
        text: text.into(),
        vector: Some(vec),
    }
}

#[tokio::test]
async fn upsert_and_search_returns_nearest() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    store
        .upsert(&[
            chunk(1, "greet", vec![1.0, 0.0, 0.0, 0.0]),
            chunk(2, "farewell", vec![0.0, 1.0, 0.0, 0.0]),
        ])
        .await
        .unwrap();
    let hits = store
        .search(&[0.99, 0.01, 0.0, 0.0], 1, &Filter::default())
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk.name.as_deref(), Some("fn1"));
}

#[tokio::test]
async fn count_reflects_upsert() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 0);
    store
        .upsert(&[chunk(3, "hi", vec![0.5, 0.5, 0.5, 0.5])])
        .await
        .unwrap();
    assert_eq!(store.count().await.unwrap(), 1);
}

#[tokio::test]
async fn get_roundtrips_chunk_by_hash() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    let original = chunk(4, "hello", vec![0.1, 0.2, 0.3, 0.4]);
    store.upsert(std::slice::from_ref(&original)).await.unwrap();
    let got = store
        .get(&ContentHash([4; 32]))
        .await
        .unwrap()
        .expect("should find chunk");
    assert_eq!(got.hash, original.hash);
    assert_eq!(got.name, original.name);
    assert_eq!(got.text, original.text);
}

#[tokio::test]
async fn delete_removes_chunks() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    store
        .upsert(&[
            chunk(5, "a", vec![1.0, 0.0, 0.0, 0.0]),
            chunk(6, "b", vec![0.0, 1.0, 0.0, 0.0]),
        ])
        .await
        .unwrap();
    assert_eq!(store.count().await.unwrap(), 2);
    store.delete(&[ContentHash([5; 32])]).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 1);
    let missing = store.get(&ContentHash([5; 32])).await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn search_honors_hash_allowlist() {
    use std::collections::HashSet;
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    store
        .upsert(&[
            chunk(7, "a", vec![1.0, 0.0, 0.0, 0.0]),
            chunk(8, "b", vec![0.99, 0.01, 0.0, 0.0]),
        ])
        .await
        .unwrap();
    // Even though [7] would be a closer hit, allowlist restricts to [8].
    let mut allow = HashSet::new();
    allow.insert(ContentHash([8; 32]));
    let filter = Filter {
        hash_allowlist: Some(allow),
        ..Default::default()
    };
    let hits = store
        .search(&[1.0, 0.0, 0.0, 0.0], 5, &filter)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk.hash, ContentHash([8; 32]));
}

#[tokio::test]
async fn upsert_rejects_wrong_dim_vector() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    let bad = chunk(10, "x", vec![1.0, 2.0]); // dim = 2, store expects 4
    let err = store.upsert(&[bad]).await.expect_err("mismatched dim");
    assert!(matches!(err, ctx_core::CtxError::Store(_)), "got: {err:?}");
}

#[tokio::test]
async fn search_rejects_wrong_query_dim() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    let err = store
        .search(&[1.0, 2.0], 1, &ctx_core::traits::Filter::default())
        .await
        .expect_err("mismatched query dim");
    assert!(matches!(err, ctx_core::CtxError::Store(_)), "got: {err:?}");
}

#[tokio::test]
async fn reopen_same_dim_preserves_rows() {
    let dir = tempdir().unwrap();
    {
        let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
        store.upsert(&[chunk(11, "persist-me", vec![0.1, 0.2, 0.3, 0.4])]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }
    // Drop and reopen
    let store2 = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    assert_eq!(store2.count().await.unwrap(), 1);
    let got = store2.get(&ContentHash([11; 32])).await.unwrap();
    assert!(got.is_some());
}

#[tokio::test]
async fn reopen_different_dim_errors() {
    let dir = tempdir().unwrap();
    {
        let _store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    }
    // Reopen with different dim — should error.
    // LanceChunkStore doesn't implement Debug so we can't use expect_err/unwrap_err;
    // match instead.
    match LanceChunkStore::open(dir.path(), 8).await {
        Err(e) => assert!(matches!(e, ctx_core::CtxError::Store(_)), "got: {e:?}"),
        Ok(_) => panic!("expected dim mismatch error, but open succeeded"),
    }
}

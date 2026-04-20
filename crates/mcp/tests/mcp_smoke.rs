//! Smoke tests for the ctx-mcp MCP server.
//!
//! Uses an in-process `tokio::io::duplex` transport (same pattern as rmcp's own
//! test suite) so there's no file I/O at the transport layer. The backing stores
//! are real (`LanceChunkStore` + `RedbRefStore`) over a temp directory; the embedder
//! is a deterministic mock so tests are fast and offline.

use async_trait::async_trait;
use ctx_core::traits::{ChunkStore as _, Embedder, RefStore as _};
use ctx_core::types::{ByteRange, Language, LineRange};
use ctx_core::{Chunk, ChunkKind, ChunkRef, ContentHash, Result, Scope, Symbol};
use ctx_mcp::CtxMcpServer;
use ctx_query::Router;
use rmcp::{
    model::{CallToolRequestParams, ClientInfo},
    ClientHandler, ServiceExt,
};
use std::sync::Arc;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// MockEmbedder — deterministic, no model download required
// ---------------------------------------------------------------------------

struct MockEmbedder;

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0_f32; 4];
                #[allow(clippy::cast_precision_loss)]
                {
                    v[0] = (t.bytes().map(u32::from).sum::<u32>() % 256) as f32;
                }
                v
            })
            .collect())
    }

    fn dim(&self) -> usize {
        4
    }

    fn model_id(&self) -> &'static str {
        "mock"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sample_chunk(n: u8, text: &str) -> Chunk {
    Chunk {
        hash: ContentHash([n; 32]),
        file: format!("src/f{n}.ts"),
        lang: Language::TypeScript,
        kind: ChunkKind::Function,
        name: Some(format!("fn{n}")),
        byte_range: ByteRange::new(0, text.len()),
        line_range: LineRange::new(0, 3),
        text: text.into(),
        vector: Some(vec![f32::from(n) * 0.1, 0.0, 0.0, 0.0]),
    }
}

fn scope_at(root: &std::path::Path) -> Scope {
    Scope::local(root, root, Some("main".into())).unwrap()
}

/// Minimal client handler (just sends requests, no server-initiated call support needed).
#[derive(Debug, Clone)]
struct DummyClient;

impl ClientHandler for DummyClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

fn extract_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_tools_returns_all_seven() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let chunks = ctx_store::LanceChunkStore::open(dir.path().join("lance"), 4).await?;
    let refs = ctx_store::RedbRefStore::open(dir.path().join("refs.redb"))?;
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let scope = scope_at(&root);
    let router = Arc::new(Router::new(
        Arc::new(chunks),
        Arc::new(refs),
        Arc::new(MockEmbedder),
    ));
    let server = CtxMcpServer::new(router, scope);

    let (server_transport, client_transport) = tokio::io::duplex(65_536);

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let tools = client.list_tools(Option::default()).await?;
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();

    let expected = [
        "semantic_search",
        "find_definition",
        "find_references",
        "find_callers",
        "get_chunk",
        "get_file_window",
        "repo_status",
    ];
    for expected_name in &expected {
        assert!(
            names.contains(expected_name),
            "missing tool '{expected_name}'; got: {names:?}"
        );
    }
    assert_eq!(
        names.len(),
        7,
        "expected exactly 7 tools, got {}",
        names.len()
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn semantic_search_returns_results() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let chunks = ctx_store::LanceChunkStore::open(dir.path().join("lance"), 4).await?;
    let refs = ctx_store::RedbRefStore::open(dir.path().join("refs.redb"))?;
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let scope = scope_at(&root);

    // Seed data
    let c1 = sample_chunk(1, "async function fetchUser(id) { return db.get(id); }");
    chunks.upsert(std::slice::from_ref(&c1)).await?;
    refs.bind(
        &scope,
        &[ChunkRef {
            hash: ContentHash([1; 32]),
            file: "src/f1.ts".into(),
            line_range: LineRange::new(0, 3),
        }],
    )
    .await?;

    let router = Arc::new(Router::new(
        Arc::new(chunks),
        Arc::new(refs),
        Arc::new(MockEmbedder),
    ));
    let server = CtxMcpServer::new(router, scope);

    let (server_transport, client_transport) = tokio::io::duplex(65_536);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("semantic_search").with_arguments(
                serde_json::json!({"query": "fetch user from database", "k": 5})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;

    let text = extract_text(&result);
    // Result should be a JSON array
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("expected JSON array, got: {text:?} — error: {e}"));
    assert!(parsed.is_array(), "expected array, got: {parsed}");
    assert!(
        !parsed.as_array().unwrap().is_empty(),
        "expected at least one hit"
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn find_definition_returns_symbol() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let chunks = ctx_store::LanceChunkStore::open(dir.path().join("lance"), 4).await?;
    let refs = ctx_store::RedbRefStore::open(dir.path().join("refs.redb"))?;
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let scope = scope_at(&root);

    refs.upsert_symbols(
        &scope,
        &[Symbol {
            name: "greetUser".into(),
            kind: ChunkKind::Function,
            file: "src/greet.ts".into(),
            line: 42,
            container: None,
        }],
    )
    .await?;

    let router = Arc::new(Router::new(
        Arc::new(chunks),
        Arc::new(refs),
        Arc::new(MockEmbedder),
    ));
    let server = CtxMcpServer::new(router, scope);

    let (server_transport, client_transport) = tokio::io::duplex(65_536);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("find_definition").with_arguments(
                serde_json::json!({"name": "greetUser"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;

    let text = extract_text(&result);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("expected JSON, got: {text:?} — error: {e}"));
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "greetUser");
    assert_eq!(arr[0]["line"], 42);

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn repo_status_returns_valid_json() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let chunks = ctx_store::LanceChunkStore::open(dir.path().join("lance"), 4).await?;
    let refs = ctx_store::RedbRefStore::open(dir.path().join("refs.redb"))?;
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let scope = scope_at(&root);

    let router = Arc::new(Router::new(
        Arc::new(chunks),
        Arc::new(refs),
        Arc::new(MockEmbedder),
    ));
    let server = CtxMcpServer::new(router, scope);

    let (server_transport, client_transport) = tokio::io::duplex(65_536);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("repo_status")
                .with_arguments(serde_json::json!({}).as_object().unwrap().clone()),
        )
        .await?;

    let text = extract_text(&result);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("expected JSON, got: {text:?} — error: {e}"));
    assert!(parsed["chunks_total"].is_number());
    assert!(parsed["active_hashes"].is_number());
    assert_eq!(parsed["embedding_model"], "mock");

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn get_chunk_with_valid_hash_returns_chunk() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let chunks = ctx_store::LanceChunkStore::open(dir.path().join("lance"), 4).await?;
    let refs = ctx_store::RedbRefStore::open(dir.path().join("refs.redb"))?;
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let scope = scope_at(&root);

    let c = sample_chunk(7, "export const CONFIG = {};");
    chunks.upsert(std::slice::from_ref(&c)).await?;

    let hash_hex = c.hash.to_hex();
    let router = Arc::new(Router::new(
        Arc::new(chunks),
        Arc::new(refs),
        Arc::new(MockEmbedder),
    ));
    let server = CtxMcpServer::new(router, scope);

    let (server_transport, client_transport) = tokio::io::duplex(65_536);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("get_chunk").with_arguments(
                serde_json::json!({"hash": hash_hex})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;

    let text = extract_text(&result);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("expected JSON, got: {text:?} — error: {e}"));
    // get_chunk returns Option<Chunk> — the Some variant serializes directly as the chunk object
    assert_eq!(parsed["file"], "src/f7.ts");

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn get_chunk_with_invalid_hash_returns_error_json() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let chunks = ctx_store::LanceChunkStore::open(dir.path().join("lance"), 4).await?;
    let refs = ctx_store::RedbRefStore::open(dir.path().join("refs.redb"))?;
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let scope = scope_at(&root);

    let router = Arc::new(Router::new(
        Arc::new(chunks),
        Arc::new(refs),
        Arc::new(MockEmbedder),
    ));
    let server = CtxMcpServer::new(router, scope);

    let (server_transport, client_transport) = tokio::io::duplex(65_536);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("get_chunk").with_arguments(
                serde_json::json!({"hash": "tooshort"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;

    let text = extract_text(&result);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("expected JSON error object, got: {text:?} — error: {e}"));
    assert!(
        parsed["error"].is_string(),
        "expected error field, got: {parsed}"
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

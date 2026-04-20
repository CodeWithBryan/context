//! MCP server implementation for ctx-mcp.
//!
//! # rmcp 1.5.0 API shape (researched 2026-04-19)
//!
//! **Version**: rmcp 1.5.0 / rmcp-macros 1.5.0, schemars 1.2.1.
//!
//! **Key traits / types**:
//! - `ServerHandler` — the core trait; blanket impl delegates all MCP
//!   request types. Default no-ops for everything except the methods you
//!   override. Implement via `#[tool_handler]` macro which auto-generates
//!   `call_tool`, `list_tools`, `get_tool`, and `get_info` based on the
//!   `ToolRouter` that `#[tool_router]` builds.
//! - `ServiceExt::serve(transport)` — drives the handshake loop, returns
//!   `RunningService`; call `.waiting().await` to block until the transport
//!   closes (normal stdio lifetime).
//! - `#[tool_router]` on `impl Server { … }` — turns each `#[tool]`-
//!   annotated `async fn` into a callable tool entry; also generates
//!   `Server::tool_router() -> ToolRouter<Self>`.
//! - `#[tool_handler]` on `impl ServerHandler for Server {}` — wires
//!   `call_tool` / `list_tools` / `get_tool` / `get_info` to the
//!   `ToolRouter` stored in `self.tool_router`.
//! - `Parameters<T>` — extractor that deserializes JSON object arguments
//!   into `T: DeserializeOwned`.
//! - `rmcp::transport::stdio()` — returns `(tokio::io::Stdin, tokio::io::Stdout)`;
//!   pass directly to `.serve()`.
//! - For in-process tests: `tokio::io::duplex(N)` yields a bidirectional
//!   async-read/write pair accepted by `.serve()`.
//!
//! **schemars note**: rmcp 1.x requires `schemars = "1"`. Tool parameter
//! structs derive `schemars::JsonSchema`. The proc-macro reads the schema at
//! compile time to populate `Tool::input_schema`.
//!
//! **Generic design**: `CtxMcpServer<C, R, E>` is generic over the three
//! store/embedder type parameters of `Router<C, R, E>`. In production (Task 12
//! CLI) use `CtxMcpServer<LanceChunkStore, RedbRefStore, FastembedEmbedder>`.
//! In tests a `MockEmbedder` is substituted to avoid model downloads.
//! The public type alias `ProductionCtxMcpServer` is provided for Task 12.

use std::sync::Arc;

use anyhow::Context as _;
use ctx_core::traits::{ChunkStore, Embedder, RefStore};
use ctx_core::{ContentHash, Scope};
use ctx_query::Router;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::instrument;

// ---------------------------------------------------------------------------
// Tool argument types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SemanticSearchArgs {
    /// Natural-language query string.
    pub query: String,
    /// Maximum number of results to return (default: 10).
    #[serde(default = "default_k")]
    pub k: u32,
}

fn default_k() -> u32 {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameArgs {
    /// Symbol or function name.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HashArgs {
    /// Hex-encoded 64-character content hash (Blake3, 32 bytes → 64 hex chars).
    pub hash: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoArgs {}

// ---------------------------------------------------------------------------
// Server struct
// ---------------------------------------------------------------------------

/// MCP server that exposes 6 ctx query tools for a single repo scope.
///
/// One server instance corresponds to one scope (one repo). Task 12 (CLI)
/// constructs this with the appropriate scope and calls `serve_stdio`.
///
/// Generic parameters mirror `Router<C, R, E>` to allow using a `MockEmbedder`
/// in tests. In production, use `ProductionCtxMcpServer`.
pub struct CtxMcpServer<C: ChunkStore, R: RefStore, E: Embedder> {
    router: Arc<Router<C, R, E>>,
    scope: Scope,
    // Used internally by the #[tool_router] / #[tool_handler] macros.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

/// Alias for the production server type.
pub type ProductionCtxMcpServer =
    CtxMcpServer<ctx_store::LanceChunkStore, ctx_store::RedbRefStore, ctx_embed::FastembedEmbedder>;

impl<C: ChunkStore + 'static, R: RefStore + 'static, E: Embedder + 'static> CtxMcpServer<C, R, E> {
    /// Create a new MCP server wrapping the given router and scope.
    #[must_use]
    pub fn new(router: Arc<Router<C, R, E>>, scope: Scope) -> Self {
        Self {
            router,
            scope,
            tool_router: Self::tool_router(),
        }
    }

    /// Serve over stdio (blocking until the transport closes).
    ///
    /// # Errors
    /// Returns an error if the MCP handshake or transport fails.
    pub async fn serve_stdio(self) -> Result<(), anyhow::Error> {
        let transport = rmcp::transport::stdio();
        self.serve(transport)
            .await
            .context("MCP handshake failed")?
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("MCP server join error: {e:?}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

#[tool_router]
impl<C: ChunkStore + 'static, R: RefStore + 'static, E: Embedder + 'static> CtxMcpServer<C, R, E> {
    /// Semantic search: find the k most relevant code chunks for a query.
    #[tool(description = "Semantic search over indexed code chunks for the current repo.")]
    #[instrument(skip(self), fields(query = %args.query, k = args.k))]
    async fn semantic_search(&self, Parameters(args): Parameters<SemanticSearchArgs>) -> String {
        match self
            .router
            .semantic_search(&self.scope, &args.query, args.k as usize)
            .await
        {
            Ok(hits) => serde_json::to_string(&hits)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Find the definition site(s) of a named symbol in the current repo.
    #[tool(description = "Find definition sites of a symbol by name in the current repo.")]
    #[instrument(skip(self), fields(name = %args.name))]
    async fn find_definition(&self, Parameters(args): Parameters<NameArgs>) -> String {
        match self.router.find_definition(&self.scope, &args.name).await {
            Ok(syms) => serde_json::to_string(&syms)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Find all reference sites of a named symbol in the current repo.
    #[tool(description = "Find all reference sites of a symbol by name in the current repo.")]
    #[instrument(skip(self), fields(name = %args.name))]
    async fn find_references(&self, Parameters(args): Parameters<NameArgs>) -> String {
        match self.router.find_references(&self.scope, &args.name).await {
            Ok(syms) => serde_json::to_string(&syms)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Find all call sites of a named function in the current repo.
    #[tool(description = "Find all callers of a function by name in the current repo.")]
    #[instrument(skip(self), fields(name = %args.name))]
    async fn find_callers(&self, Parameters(args): Parameters<NameArgs>) -> String {
        match self.router.find_callers(&self.scope, &args.name).await {
            Ok(syms) => serde_json::to_string(&syms)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Retrieve a single chunk by its hex-encoded Blake3 content hash.
    #[tool(
        description = "Retrieve a code chunk by its 64-character hex-encoded Blake3 content hash."
    )]
    #[instrument(skip(self), fields(hash = %args.hash))]
    async fn get_chunk(&self, Parameters(args): Parameters<HashArgs>) -> String {
        let hash = match parse_hex_hash(&args.hash) {
            Ok(h) => h,
            Err(msg) => return serde_json::json!({"error": msg}).to_string(),
        };
        match self.router.get_chunk(hash).await {
            Ok(chunk) => serde_json::to_string(&chunk)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Return indexing status for the current repo scope.
    #[tool(description = "Return indexing status (chunk counts, model info) for the current repo.")]
    #[instrument(skip(self))]
    async fn repo_status(&self, Parameters(_): Parameters<NoArgs>) -> String {
        match self.router.status(&self.scope).await {
            Ok(status) => serde_json::to_string(&status)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }
}

#[tool_handler]
impl<C: ChunkStore + 'static, R: RefStore + 'static, E: Embedder + 'static> ServerHandler
    for CtxMcpServer<C, R, E>
{
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn parse_hex_hash(hex: &str) -> Result<ContentHash, String> {
    if hex.len() != 64 {
        return Err(format!("hash must be 64 hex chars (got {})", hex.len()));
    }
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        let hi = hex_nibble(hex.as_bytes()[i * 2])?;
        let lo = hex_nibble(hex.as_bytes()[i * 2 + 1])?;
        *b = (hi << 4) | lo;
    }
    Ok(ContentHash(bytes))
}

fn hex_nibble(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!("invalid hex character: {}", c as char)),
    }
}

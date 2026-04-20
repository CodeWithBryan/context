//! ctx-mcp — MCP server exposing 6 ctx query tools over stdio.
//!
//! Uses rmcp 1.5.0. See `server.rs` for the full API shape documentation.
//!
//! The server exposes these tools (all scoped to a single repo passed at
//! construction time):
//!
//! | Tool | Arguments | Returns |
//! |------|-----------|---------|
//! | `semantic_search` | `query: String, k: u32 = 10` | JSON `Vec<Hit>` |
//! | `find_definition` | `name: String` | JSON `Vec<Symbol>` |
//! | `find_references` | `name: String` | JSON `Vec<Symbol>` |
//! | `find_callers`    | `name: String` | JSON `Vec<Symbol>` |
//! | `get_chunk`       | `hash: String` (64-char hex) | JSON `Option<Chunk>` |
//! | `repo_status`     | *(none)* | JSON `Status` |

mod server;

pub use server::{CtxMcpServer, ProductionCtxMcpServer};
pub use ctx_query::Status;

/// LSP (Language Server Protocol) client for symbol extraction.
///
/// Implements a generic JSON-RPC 2.0 / LSP client that communicates with
/// language servers over stdio using `Content-Length:`-framed messages.
///
/// Phase 1 supports:
/// - `initialize` / `initialized`
/// - `textDocument/didOpen`
/// - `textDocument/documentSymbol`
/// - `shutdown` / `exit`
///
/// Phase 2+ will add `textDocument/definition`, `references`, `completions`, etc.
pub mod client;
pub mod launchers;

pub use client::{LspClient, Url};

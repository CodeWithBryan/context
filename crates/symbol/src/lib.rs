//! ctx-symbol — symbol extraction via tree-sitter (CSS/HTML) and LSP (TS/JS). Implemented in Task 7.
//!
//! The `tsserver` module contains the legacy tsserver bridge and is deprecated
//! in favour of `lsp`. It is retained for existing tests but is no longer wired
//! into the indexing pipeline.

pub mod extractor;
pub mod lsp;
pub mod tree_symbols;
/// Deprecated: use `ctx_symbol::lsp` instead.
///
/// The classic tsserver protocol bridge. Retained for existing tests but no
/// longer wired into the pipeline. Scheduled for removal in a future task.
pub mod tsserver;

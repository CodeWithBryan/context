/// LSP server launchers. Each sub-module knows how to locate and spawn a
/// specific language server.
///
/// Phase 1 ships only `tsgo`. Future phases will add rust-analyzer, gopls,
/// pyright, etc.
pub mod tsgo;

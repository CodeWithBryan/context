use crate::lsp::client::LspClient;
use ctx_core::Result;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::warn;

/// Try to spawn a `tsgo` LSP server for the given project root.
///
/// Returns:
/// - `Ok(Some(LspClient))` — tsgo found and initialized successfully.
/// - `Ok(None)` — tsgo not present; TS symbol extraction disabled (graceful).
/// - `Err(...)` — tsgo found but spawn/initialize failed unexpectedly.
pub async fn try_spawn(project_root: &Path) -> Result<Option<LspClient>> {
    let Some(tsgo_path) = resolve_tsgo_path(project_root) else {
        warn!("tsgo not found in {project_root:?} — TS LSP symbol extraction disabled");
        return Ok(None);
    };

    // tsgo speaks LSP over stdio when invoked with `--lsp --stdio`
    let mut cmd = Command::new(&tsgo_path);
    cmd.arg("--lsp").arg("--stdio").current_dir(project_root);

    let client = LspClient::spawn(cmd, "tsgo")?;
    client.initialize(project_root).await?;
    Ok(Some(client))
}

/// Resolve the tsgo binary path for a project.
///
/// Search order:
/// 1. `<project_root>/node_modules/.bin/tsgo` (project-local wrapper)
/// 2. `CTX_TSGO_PATH` environment variable override
fn resolve_tsgo_path(project_root: &Path) -> Option<PathBuf> {
    // Prefer the project-local wrapper (handles platform-specific binary selection)
    let local = project_root.join("node_modules/.bin/tsgo");
    if local.exists() {
        return Some(local);
    }

    // Env-var override (e.g. for CI or global installs)
    if let Ok(override_path) = std::env::var("CTX_TSGO_PATH") {
        let p = PathBuf::from(override_path);
        if p.exists() {
            return Some(p);
        }
        warn!("CTX_TSGO_PATH set but does not exist: {}", p.display());
    }

    None
}

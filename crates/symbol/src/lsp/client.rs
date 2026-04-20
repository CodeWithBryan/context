use ctx_core::{ChunkKind, CtxError, Result, Symbol};
use lsp_types::{
    ClientCapabilities, DocumentSymbol, DocumentSymbolClientCapabilities, DocumentSymbolResponse,
    InitializeParams, InitializedParams, SymbolKind, TextDocumentClientCapabilities,
    TextDocumentSyncClientCapabilities,
};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

// Re-export url::Url so callers can build file:// URIs via `Url::from_file_path`.
pub use url::Url;

/// Generic LSP client that communicates over a child process's stdin/stdout
/// using JSON-RPC 2.0 messages framed with `Content-Length:` headers.
pub struct LspClient {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    request_id: AtomicI64,
    /// Serializes entire write→read cycles so concurrent callers don't
    /// interleave responses.
    request_lock: Mutex<()>,
    server_name: String,
}

impl LspClient {
    /// Spawn a child process running the given command with stdin/stdout piped.
    /// Does NOT perform the LSP `initialize` handshake — call `initialize` next.
    pub fn spawn(mut cmd: Command, server_name: impl Into<String>) -> Result<Self> {
        let server_name = server_name.into();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| CtxError::Symbol(format!("spawn {server_name}: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CtxError::Symbol(format!("{server_name}: no stdin")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CtxError::Symbol(format!("{server_name}: no stdout")))?;
        // Forward server stderr to tracing::debug
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CtxError::Symbol(format!("{server_name}: no stderr")))?;
        let sname = server_name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                debug!("{sname} stderr: {}", line.trim());
                line.clear();
            }
        });
        Ok(Self {
            child: Mutex::new(child),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            request_id: AtomicI64::new(1),
            request_lock: Mutex::new(()),
            server_name,
        })
    }

    /// Send LSP `initialize` + `initialized` notification.
    /// MUST be called before any other request.
    pub async fn initialize(&self, root: &Path) -> Result<()> {
        // Build a file:// URI string for the root directory, then parse as lsp_types::Uri
        let root_file_url = Url::from_directory_path(root)
            .map_err(|()| CtxError::Symbol(format!("invalid root path: {}", root.display())))?;
        let parsed_root_uri = lsp_types::Uri::from_str(root_file_url.as_str())
            .map_err(|e| CtxError::Symbol(format!("parse root URI: {e}")))?;

        #[allow(deprecated)]
        // root_uri is deprecated in favor of workspace_folders, but widely supported
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(parsed_root_uri),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        dynamic_registration: Some(false),
                        ..Default::default()
                    }),
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let params_value = serde_json::to_value(params)
            .map_err(|e| CtxError::Symbol(format!("serialize initialize params: {e}")))?;
        let _resp = self.request("initialize", params_value).await?;

        // Send the `initialized` notification (no response expected)
        let init_params = serde_json::to_value(InitializedParams {})
            .map_err(|e| CtxError::Symbol(format!("serialize initialized params: {e}")))?;
        self.notify("initialized", init_params).await?;

        Ok(())
    }

    /// Send a `textDocument/didOpen` notification with the file contents.
    pub async fn did_open(&self, uri: &Url, language_id: &str, text: &str) -> Result<()> {
        // Build the params JSON directly to avoid lsp_types::Uri type friction
        let params = json!({
            "textDocument": {
                "uri": uri.as_str(),
                "languageId": language_id,
                "version": 1,
                "text": text
            }
        });
        self.notify("textDocument/didOpen", params).await
    }

    /// Request document symbols for a given file URI.
    /// Returns a flat list of `ctx_core::Symbol`s.
    pub async fn document_symbols(&self, uri: &Url, file_path: &str) -> Result<Vec<Symbol>> {
        // Build params directly to avoid lsp_types::Uri type friction
        let params = json!({
            "textDocument": { "uri": uri.as_str() }
        });

        let resp = self.request("textDocument/documentSymbol", params).await?;

        if resp.is_null() {
            return Ok(vec![]);
        }

        let doc_sym_resp: Option<DocumentSymbolResponse> =
            serde_json::from_value(resp).unwrap_or(None);

        let mut out = Vec::new();
        match doc_sym_resp {
            Some(DocumentSymbolResponse::Nested(items)) => {
                flatten_document_symbols(items, file_path, None, &mut out);
            }
            Some(DocumentSymbolResponse::Flat(items)) => {
                for item in items {
                    if let Some(kind) = map_lsp_symbol_kind(item.kind) {
                        let line = item.location.range.start.line.saturating_add(1);
                        out.push(Symbol {
                            name: item.name,
                            kind,
                            file: file_path.to_string(),
                            line,
                            container: item.container_name,
                        });
                    }
                }
            }
            None => {}
        }

        Ok(out)
    }

    /// Gracefully shut down the LSP server and kill the child process.
    pub async fn shutdown(&self) -> Result<()> {
        // Best-effort shutdown request
        if let Err(e) = self.request("shutdown", Value::Null).await {
            warn!("{} shutdown request failed: {e}", self.server_name);
        }
        // Send exit notification (no response)
        let _ = self.notify("exit", Value::Null).await;
        // Kill the child process
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }

    // ── Low-level helpers ────────────────────────────────────────────────────

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        // Acquire the gate for the full write→read cycle
        let _gate = self.request_lock.lock().await;

        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let msg = if params.is_null() {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            })
        };
        self.write_message(&msg).await?;

        // Read responses until we find one matching our request id
        let server_name = self.server_name.clone();
        let fut = async {
            loop {
                let val = self.read_message().await?;
                // Check if it's a response (has "id") vs notification
                let msg_id = val.get("id").and_then(Value::as_i64);
                if let Some(resp_id) = msg_id {
                    if resp_id == id {
                        // Check for LSP error
                        if let Some(err) = val.get("error") {
                            return Err(CtxError::Symbol(format!(
                                "{server_name} LSP error for {method}: {err}"
                            )));
                        }
                        let result = val.get("result").cloned().unwrap_or(Value::Null);
                        return Ok(result);
                    }
                    debug!("{server_name}: skipping response id={resp_id} (waiting for {id})");
                } else {
                    // It's a notification — discard
                    debug!(
                        "{server_name}: notification: {}",
                        val.get("method").and_then(|v| v.as_str()).unwrap_or("?")
                    );
                }
            }
        };

        timeout(Duration::from_secs(15), fut).await.map_err(|_| {
            CtxError::Symbol(format!(
                "{} timeout waiting for response to {method}",
                self.server_name
            ))
        })?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = if params.is_null() {
            json!({
                "jsonrpc": "2.0",
                "method": method
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params
            })
        };
        self.write_message(&msg).await
    }

    async fn write_message(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)
            .map_err(|e| CtxError::Symbol(format!("serialize LSP message: {e}")))?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(header.as_bytes()).await.map_err(|e| {
            CtxError::Symbol(format!("{} stdin write header: {e}", self.server_name))
        })?;
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| CtxError::Symbol(format!("{} stdin write body: {e}", self.server_name)))?;
        stdin
            .flush()
            .await
            .map_err(|e| CtxError::Symbol(format!("{} stdin flush: {e}", self.server_name)))?;
        Ok(())
    }

    async fn read_message(&self) -> Result<Value> {
        let mut stdout = self.stdout.lock().await;
        // Read headers until blank line; extract Content-Length
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = stdout.read_line(&mut line).await.map_err(|e| {
                CtxError::Symbol(format!("{} stdout read header: {e}", self.server_name))
            })?;
            if n == 0 {
                return Err(CtxError::Symbol(format!(
                    "{} server closed",
                    self.server_name
                )));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                // End of headers
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                let len: usize = rest.trim().parse().map_err(|_| {
                    CtxError::Symbol(format!(
                        "{} invalid Content-Length: {rest}",
                        self.server_name
                    ))
                })?;
                content_length = Some(len);
            }
            // Ignore other headers (e.g. Content-Type)
        }
        let len = content_length.ok_or_else(|| {
            CtxError::Symbol(format!("{} missing Content-Length", self.server_name))
        })?;
        let mut body = vec![0u8; len];
        stdout
            .read_exact(&mut body)
            .await
            .map_err(|e| CtxError::Symbol(format!("{} stdout read body: {e}", self.server_name)))?;
        let val: Value = serde_json::from_slice(&body)
            .map_err(|e| CtxError::Symbol(format!("{} JSON parse: {e}", self.server_name)))?;
        Ok(val)
    }
}

// ── Symbol mapping ────────────────────────────────────────────────────────────

fn map_lsp_symbol_kind(k: SymbolKind) -> Option<ChunkKind> {
    Some(match k {
        SymbolKind::FUNCTION | SymbolKind::OPERATOR => ChunkKind::Function,
        SymbolKind::METHOD | SymbolKind::CONSTRUCTOR => ChunkKind::Method,
        SymbolKind::CLASS => ChunkKind::Class,
        SymbolKind::INTERFACE => ChunkKind::Interface,
        SymbolKind::TYPE_PARAMETER | SymbolKind::STRUCT => ChunkKind::Type,
        SymbolKind::CONSTANT | SymbolKind::VARIABLE | SymbolKind::FIELD | SymbolKind::PROPERTY => {
            ChunkKind::Const
        }
        SymbolKind::ENUM | SymbolKind::ENUM_MEMBER => ChunkKind::Enum,
        _ => return None,
    })
}

fn flatten_document_symbols(
    items: Vec<DocumentSymbol>,
    file: &str,
    container: Option<&str>,
    out: &mut Vec<Symbol>,
) {
    for item in items {
        // LSP uses 0-indexed lines; our Symbol.line is 1-indexed
        let line = item.range.start.line.saturating_add(1);
        if let Some(kind) = map_lsp_symbol_kind(item.kind) {
            out.push(Symbol {
                name: item.name.clone(),
                kind,
                file: file.to_string(),
                line,
                container: container.map(str::to_owned),
            });
        }
        #[allow(deprecated)]
        // lsp_types marks DocumentSymbol.children as non-deprecated; suppress false positive
        if let Some(children) = item.children {
            flatten_document_symbols(children, file, Some(&item.name), out);
        }
    }
}

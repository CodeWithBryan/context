use ctx_core::{ChunkKind, CtxError, Result, Symbol};
use lsp_types::{
    ClientCapabilities, DocumentSymbol, DocumentSymbolClientCapabilities, DocumentSymbolResponse,
    InitializeParams, InitializedParams, SymbolKind, TextDocumentClientCapabilities,
    TextDocumentSyncClientCapabilities,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::{timeout, Duration};
use tracing::{debug, trace, warn};

// Re-export url::Url so callers can build file:// URIs via `Url::from_file_path`.
pub use url::Url;

/// Pending requests: maps request id → oneshot sender for the response value.
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;

/// Generic LSP client that communicates over a child process's stdin/stdout
/// using JSON-RPC 2.0 messages framed with `Content-Length:` headers.
///
/// Uses an async dispatcher model: a background reader task classifies every
/// incoming message and either fulfills a pending request via a oneshot channel
/// or auto-replies to server-initiated requests.  This eliminates the
/// write→read lock-step pattern that caused deadlocks when the server sent
/// `client/registerCapability`, `window/workDoneProgress/create`, or
/// `workspace/configuration` requests before answering ours.
pub struct LspClient {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    request_id: AtomicI64,
    pending: Pending,
    /// Kept alive so the reader task lives as long as the client does.
    _reader: tokio::task::JoinHandle<()>,
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
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CtxError::Symbol(format!("{server_name}: no stderr")))?;

        // Forward server stderr to tracing::debug
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

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let stdin_arc = Arc::new(Mutex::new(stdin));

        // Spawn the reader / dispatcher task.
        let reader_pending = Arc::clone(&pending);
        let reader_stdin = Arc::clone(&stdin_arc);
        let reader_server_name = server_name.clone();
        let reader_handle = tokio::spawn(async move {
            reader_task(
                BufReader::new(stdout),
                reader_stdin,
                reader_pending,
                reader_server_name,
            )
            .await;
        });

        Ok(Self {
            child: Mutex::new(child),
            stdin: stdin_arc,
            request_id: AtomicI64::new(1),
            pending,
            _reader: reader_handle,
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

        // Register the oneshot *before* sending to avoid a race where the
        // reader task delivers the response before we've registered.
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        trace!("{} → request id={id} method={method}", self.server_name);
        if let Err(e) = self.write_message(&msg).await {
            // Clean up the pending entry if send failed.
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        let server_name = self.server_name.clone();
        timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                CtxError::Symbol(format!(
                    "{server_name} timeout waiting for response to {method}"
                ))
            })?
            .map_err(|_| {
                CtxError::Symbol(format!(
                    "{server_name} reader task dropped while waiting for {method}"
                ))
            })
            .and_then(|val| {
                if let Some(err) = val.get("error") {
                    Err(CtxError::Symbol(format!(
                        "{server_name} LSP error for {method}: {err}"
                    )))
                } else {
                    Ok(val.get("result").cloned().unwrap_or(Value::Null))
                }
            })
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
        trace!("{} → notify method={method}", self.server_name);
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
}

// ── Reader / dispatcher task ──────────────────────────────────────────────────

/// Reads framed JSON-RPC messages from the server's stdout and dispatches them:
///
/// - **Response** (has `"id"`, no `"method"`): fulfils the matching pending
///   oneshot.
/// - **Server-initiated request** (has `"id"` AND `"method"`): auto-replies
///   so the server is never left waiting.
/// - **Notification** (has `"method"`, no `"id"`): logged and discarded.
async fn reader_task(
    mut stdout: BufReader<ChildStdout>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    server_name: String,
) {
    loop {
        let val = match read_framed_message(&mut stdout, &server_name).await {
            Ok(v) => v,
            Err(e) => {
                debug!("{server_name} reader exiting: {e}");
                break;
            }
        };

        let has_method = val.get("method").is_some();
        let has_id = val.get("id").is_some();

        if has_method && has_id {
            // Server-initiated request — must reply or the server deadlocks.
            let method = val
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned();
            let req_id = val.get("id").cloned().unwrap_or(Value::Null);
            trace!("{server_name} ← server request id={req_id} method={method}");
            auto_reply(&stdin, &server_name, req_id, &method, &val).await;
        } else if has_method {
            // Notification — log and move on.
            let method = val.get("method").and_then(Value::as_str).unwrap_or("?");
            debug!("{server_name} ← notification method={method}");
        } else if let Some(id) = val.get("id").and_then(Value::as_i64) {
            // Response to one of our requests.
            let method_hint = val
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("(response)");
            trace!("{server_name} ← response id={id} method={method_hint}");
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(val);
            } else {
                warn!("{server_name}: received response for unknown id={id}");
            }
        } else {
            debug!("{server_name}: received unclassifiable message: {val}");
        }
    }

    // Reader exited — drop all pending senders so callers unblock with RecvError.
    pending.lock().await.clear();
}

/// Send a response to a server-initiated request.
async fn auto_reply(
    stdin: &Arc<Mutex<ChildStdin>>,
    server_name: &str,
    req_id: Value,
    method: &str,
    _req: &Value,
) {
    let result = match method {
        // These are all safe to acknowledge with null / empty results.
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => Value::Null,

        // workspace/configuration — reply with an empty array (the server sends
        // a list of config items it wants; we have none to provide).
        "workspace/configuration" => json!([]),

        // Unknown server→client request — reply with Method Not Found so the
        // server knows not to wait forever.
        _ => {
            warn!("{server_name}: unknown server request method={method} — replying with error");
            let response = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": { "code": -32601, "message": "Method not found" }
            });
            write_framed(stdin, server_name, &response).await;
            return;
        }
    };

    let response = json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": result
    });
    write_framed(stdin, server_name, &response).await;
}

/// Write a single framed JSON-RPC message to stdin.
async fn write_framed(stdin: &Arc<Mutex<ChildStdin>>, server_name: &str, msg: &Value) {
    let Ok(body) = serde_json::to_string(msg) else {
        warn!("{server_name}: failed to serialize auto-reply");
        return;
    };
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut guard = stdin.lock().await;
    if guard.write_all(header.as_bytes()).await.is_err()
        || guard.write_all(body.as_bytes()).await.is_err()
        || guard.flush().await.is_err()
    {
        debug!("{server_name}: auto-reply write failed (server may have closed)");
    }
}

/// Read a single `Content-Length`-framed JSON-RPC message from any async
/// buffered reader.  Made generic so the framing logic can be tested with
/// in-process `tokio::io::duplex` streams without needing a real child process.
pub(crate) async fn read_framed_message<R>(reader: &mut R, server_name: &str) -> Result<Value>
where
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
{
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| CtxError::Symbol(format!("{server_name} stdout read header: {e}")))?;
        if n == 0 {
            return Err(CtxError::Symbol(format!("{server_name} server closed")));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // End of headers
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            let len: usize = rest.trim().parse().map_err(|_| {
                CtxError::Symbol(format!("{server_name} invalid Content-Length: {rest}"))
            })?;
            content_length = Some(len);
        }
        // Ignore other headers (e.g. Content-Type)
    }
    let len = content_length
        .ok_or_else(|| CtxError::Symbol(format!("{server_name} missing Content-Length")))?;
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| CtxError::Symbol(format!("{server_name} stdout read body: {e}")))?;
    let val: Value = serde_json::from_slice(&body)
        .map_err(|e| CtxError::Symbol(format!("{server_name} JSON parse: {e}")))?;
    Ok(val)
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

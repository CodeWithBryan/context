use ctx_core::{ChunkKind, CtxError, Result, Symbol};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

pub struct TsServer {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    seq: AtomicI64,
    /// Serializes entire write→read cycles so concurrent callers don't
    /// interleave responses (Fix 2).
    request_lock: Mutex<()>,
}

impl TsServer {
    /// Spawn a tsserver child process rooted at `project_root`.
    ///
    /// Returns `Err` if tsserver cannot be found or the process fails to start.
    pub fn spawn(project_root: &Path) -> Result<Self> {
        let tsserver_path = resolve_tsserver_path(project_root)?;
        let mut cmd = Command::new("node");
        cmd.arg(&tsserver_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()) // Fix 6: pipe stderr for debuggability
            .current_dir(project_root);
        let mut child =
            cmd.spawn().map_err(|e| CtxError::Symbol(format!("spawn node: {e}")))?;
        let stdin =
            child.stdin.take().ok_or_else(|| CtxError::Symbol("no stdin".into()))?;
        let stdout =
            child.stdout.take().ok_or_else(|| CtxError::Symbol("no stdout".into()))?;
        // Fix 6: forward tsserver stderr lines to tracing::debug!
        let stderr =
            child.stderr.take().ok_or_else(|| CtxError::Symbol("no stderr".into()))?;
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                debug!("tsserver stderr: {}", line.trim());
                line.clear();
            }
        });
        Ok(Self {
            child: Mutex::new(child),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            seq: AtomicI64::new(1),
            request_lock: Mutex::new(()),
        })
    }

    /// Try to spawn a `TsServer` for the given project. Returns `Ok(None)` if
    /// tsserver is not available (logged as a warning); returns `Ok(Some(...))`
    /// if successful; returns `Err` only on genuine unexpected failures (e.g.
    /// Node process spawn failed for reasons other than missing binary).
    // `async` kept intentionally: callers use `.await` and future versions
    // may perform async work (e.g. waiting for the process to be ready).
    #[allow(clippy::unused_async)]
    pub async fn try_spawn(project_root: &Path) -> Result<Option<TsServer>> {
        match TsServer::spawn(project_root) {
            Ok(server) => Ok(Some(server)),
            Err(CtxError::Symbol(msg)) if msg.starts_with("tsserver not found") => {
                warn!("tsserver unavailable: {msg} — TS structural queries disabled");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Send a notification (no response expected). Fix 1.
    async fn notify(&self, command: &str, arguments: Value) -> Result<()> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let msg = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments
        });
        let req_bytes = format!("{msg}\n").into_bytes();
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&req_bytes)
            .await
            .map_err(|e| CtxError::Symbol(format!("stdin write: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| CtxError::Symbol(format!("stdin flush: {e}")))?;
        Ok(())
    }

    async fn request(&self, command: &str, arguments: Value) -> Result<Value> {
        // Fix 2: acquire gate for the full write→read cycle to prevent
        // concurrent requests from interleaving responses.
        let _gate = self.request_lock.lock().await;

        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let msg = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments
        });
        let req_bytes = format!("{msg}\n").into_bytes();
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(&req_bytes)
                .await
                .map_err(|e| CtxError::Symbol(format!("stdin write: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| CtxError::Symbol(format!("stdin flush: {e}")))?;
        }
        let fut = async {
            loop {
                let mut line = String::new();
                {
                    let mut stdout = self.stdout.lock().await;
                    let n = stdout
                        .read_line(&mut line)
                        .await
                        .map_err(|e| CtxError::Symbol(format!("stdout read: {e}")))?;
                    if n == 0 {
                        return Err(CtxError::Symbol("tsserver closed".into()));
                    }
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let val: Value = if let Ok(v) = serde_json::from_str(trimmed) {
                    v
                } else {
                    debug!("non-json line: {trimmed}");
                    continue;
                };
                let msg_type = val.get("type").and_then(Value::as_str).unwrap_or("");
                if msg_type != "response" {
                    continue;
                }
                let resp_seq = val.get("request_seq").and_then(Value::as_i64).unwrap_or(-1);
                if resp_seq != seq {
                    continue;
                }
                return Ok(val);
            }
        };
        timeout(Duration::from_secs(15), fut)
            .await
            .map_err(|_| CtxError::Symbol("tsserver timeout".into()))?
    }

    /// Fix 1: `open` is a tsserver notification — no response is expected.
    pub async fn open(&self, file: &Path) -> Result<()> {
        self.notify("open", json!({ "file": file })).await
    }

    pub async fn navtree(&self, file: &Path) -> Result<Vec<Symbol>> {
        // Make sure the file is open
        self.open(file).await?;
        let resp = self.request("navtree", json!({ "file": file })).await?;
        let body = resp.get("body");
        let Some(body) = body else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        collect_navtree(body, file, None, &mut out);
        Ok(out)
    }

    pub async fn definition(&self, file: &Path, line: u32, offset: u32) -> Result<Vec<Symbol>> {
        self.open(file).await?;
        let resp = self
            .request(
                "definition",
                json!({
                    "file": file,
                    "line": line,
                    "offset": offset
                }),
            )
            .await?;
        let mut out = Vec::new();
        if let Some(body) = resp.get("body").and_then(Value::as_array) {
            for item in body {
                if let (Some(file_str), Some(start)) =
                    (item.get("file").and_then(Value::as_str), item.get("start"))
                {
                    let sym_line = u32::try_from(
                        start.get("line").and_then(Value::as_u64).unwrap_or(0),
                    )
                    .unwrap_or(u32::MAX);
                    out.push(Symbol {
                        name: String::new(),
                        kind: ChunkKind::Function,
                        file: file_str.to_string(),
                        line: sym_line,
                        container: None,
                    });
                }
            }
        }
        Ok(out)
    }

    pub async fn references(&self, file: &Path, line: u32, offset: u32) -> Result<Vec<Symbol>> {
        self.open(file).await?;
        let resp = self
            .request(
                "references",
                json!({
                    "file": file,
                    "line": line,
                    "offset": offset
                }),
            )
            .await?;
        let mut out = Vec::new();
        if let Some(body) = resp
            .get("body")
            .and_then(|b| b.get("refs"))
            .and_then(Value::as_array)
        {
            for item in body {
                if let (Some(file_str), Some(start)) =
                    (item.get("file").and_then(Value::as_str), item.get("start"))
                {
                    let sym_line = u32::try_from(
                        start.get("line").and_then(Value::as_u64).unwrap_or(0),
                    )
                    .unwrap_or(u32::MAX);
                    out.push(Symbol {
                        name: String::new(),
                        kind: ChunkKind::Function,
                        file: file_str.to_string(),
                        line: sym_line,
                        container: None,
                    });
                }
            }
        }
        Ok(out)
    }

    pub async fn shutdown(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }
}

fn collect_navtree(
    node: &Value,
    file: &Path,
    container: Option<&String>,
    out: &mut Vec<Symbol>,
) {
    let text = node.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let kind_str = node.get("kind").and_then(Value::as_str).unwrap_or("");
    let kind = map_tsserver_kind(kind_str);
    let line = u32::try_from(
        node.get("spans")
            .and_then(Value::as_array)
            .and_then(|spans| spans.first())
            .and_then(|s| s.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )
    .unwrap_or(u32::MAX);
    if !text.is_empty() && text != "<global>" {
        if let Some(k) = kind {
            out.push(Symbol {
                name: text.clone(),
                kind: k,
                file: file.to_string_lossy().into_owned(),
                line,
                container: container.cloned(),
            });
        }
    }
    if let Some(children) = node.get("childItems").and_then(Value::as_array) {
        let inner: Option<String> = if text.is_empty() {
            container.cloned()
        } else {
            Some(text)
        };
        for c in children {
            collect_navtree(c, file, inner.as_ref(), out);
        }
    }
}

fn map_tsserver_kind(k: &str) -> Option<ChunkKind> {
    match k {
        "function" | "local function" => Some(ChunkKind::Function),
        "method" => Some(ChunkKind::Method),
        "class" => Some(ChunkKind::Class),
        "interface" => Some(ChunkKind::Interface),
        "type" | "type parameter" => Some(ChunkKind::Type),
        "const" | "let" | "var" => Some(ChunkKind::Const),
        "enum" => Some(ChunkKind::Enum),
        _ => None,
    }
}

/// Fix 3: check project-local tsserver first, fall back to env var override.
fn resolve_tsserver_path(project_root: &Path) -> Result<PathBuf> {
    let candidates = [
        project_root.join("node_modules/typescript/bin/tsserver"),
        project_root.join("node_modules/.bin/tsserver"),
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    // Fall back to explicit override
    if let Ok(override_path) = std::env::var("CTX_TSSERVER_PATH") {
        let p = PathBuf::from(override_path);
        if p.exists() {
            return Ok(p);
        }
        warn!("CTX_TSSERVER_PATH set but does not exist: {}", p.display());
    }
    Err(CtxError::Symbol(format!(
        "tsserver not found (tried: {}, set CTX_TSSERVER_PATH to override)",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

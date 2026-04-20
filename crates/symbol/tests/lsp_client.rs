//! Tests for `ctx_symbol::lsp::client`.
//!
//! - `framing_roundtrip` — pure in-process test of `Content-Length` framing.
//!   No real LSP server required.  Always runs.
//!
//! - `tsgo_document_symbols` — spawns a real `tsgo` process against a tiny
//!   TypeScript fixture and asserts that `greet` appears in the returned
//!   symbols.  Requires tsgo.  Marked `#[ignore]`; run with:
//!
//!   ```sh
//!   cargo test --ignored -p ctx-symbol lsp_client -- --nocapture
//!   ```
//!
//! tsgo is located via `CTX_TSGO_PATH` env var or by looking inside the
//! hotwash checkout at `~/Development/hotwash/hotwash/node_modules/.bin/tsgo`.

use std::path::PathBuf;
use std::time::Instant;

use ctx_symbol::lsp::{LspClient, Url};
use serde_json::{json, Value};
use tokio::io::{AsyncWriteExt, BufReader};

// ── Framing helpers (duplicated from client internals for the test) ───────────

/// Encode a JSON-RPC value as a `Content-Length:`-framed byte string.
fn encode_framed(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_string(msg).expect("serialize");
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut out = header.into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Inline Content-Length framing reader for the test (no dependency on
/// `pub(crate)` internals — verifies the *protocol*, not the private function).
async fn read_framed(reader: &mut BufReader<tokio::io::DuplexStream>) -> Option<Value> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.expect("read line");
        if n == 0 {
            return None; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            let len: usize = rest.trim().parse().expect("parse length");
            content_length = Some(len);
        }
    }
    let len = content_length.expect("Content-Length header");
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.expect("read body");
    let v: Value = serde_json::from_slice(&body).expect("parse json");
    Some(v)
}

// ── Test: framing round-trip (no LSP server) ─────────────────────────────────

#[tokio::test]
async fn framing_roundtrip() {
    // Build two messages to frame and read back.
    let msg1 = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "processId": null }
    });
    let msg2 = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "capabilities": {} }
    });

    // Write both into a duplex stream.
    let (mut writer_half, reader_half) = tokio::io::duplex(65_536);

    // Encode and write
    let bytes1 = encode_framed(&msg1);
    let bytes2 = encode_framed(&msg2);
    writer_half.write_all(&bytes1).await.expect("write msg1");
    writer_half.write_all(&bytes2).await.expect("write msg2");
    // Close the writer side so the reader's read_line sees EOF after the
    // second message instead of blocking forever.
    drop(writer_half);

    // Read back both messages using the inline framing reader below.
    let mut buf_reader = BufReader::new(reader_half);

    // First message
    let got1 = read_framed(&mut buf_reader).await.expect("msg1");
    assert_eq!(got1["id"], 1);
    assert_eq!(got1["method"], "initialize");

    // Second message
    let got2 = read_framed(&mut buf_reader).await.expect("msg2");
    assert_eq!(got2["id"], 1);
    assert!(got2.get("result").is_some(), "expected result in msg2");

    // EOF — no third message
    let got3 = read_framed(&mut buf_reader).await;
    assert!(got3.is_none(), "expected EOF, got {got3:?}");
}

// ── Test: real tsgo integration (requires tsgo binary) ───────────────────────

/// Locate `tsgo` for use in the integration test.
///
/// Search order:
/// 1. `CTX_TSGO_PATH` environment variable.
/// 2. `~/Development/hotwash/hotwash/node_modules/.bin/tsgo`.
fn find_tsgo() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CTX_TSGO_PATH") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let hotwash = dirs::home_dir()?.join("Development/hotwash/hotwash/node_modules/.bin/tsgo");
    if hotwash.exists() {
        return Some(hotwash);
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires tsgo; run with: cargo test --ignored -p ctx-symbol lsp_client -- --nocapture"]
async fn tsgo_document_symbols() {
    // Turn on tracing for this test so we can see what tsgo sends.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ctx_symbol=trace")),
        )
        .try_init();

    let tsgo_path = find_tsgo().expect(
        "tsgo not found — set CTX_TSGO_PATH or place tsgo at \
         ~/Development/hotwash/hotwash/node_modules/.bin/tsgo",
    );
    eprintln!("tsgo: {}", tsgo_path.display());

    // Fixture lives at tests/fixtures/ts_project/
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ts_project");
    assert!(
        fixture.exists(),
        "fixture directory not found: {}",
        fixture.display()
    );
    let a_ts = fixture.join("a.ts");
    assert!(a_ts.exists(), "fixture a.ts not found: {}", a_ts.display());

    let wall = Instant::now();

    // Spawn tsgo
    let mut cmd = tokio::process::Command::new(&tsgo_path);
    cmd.arg("--lsp").arg("--stdio").current_dir(&fixture);

    let client = LspClient::spawn(cmd, "tsgo-test").expect("spawn tsgo");

    // initialize
    client
        .initialize(&fixture)
        .await
        .expect("initialize failed");
    eprintln!("initialize: OK ({:?})", wall.elapsed());

    // didOpen
    let file_url = Url::from_file_path(&a_ts).expect("file url");
    let text = std::fs::read_to_string(&a_ts).expect("read a.ts");
    client
        .did_open(&file_url, "typescript", &text)
        .await
        .expect("didOpen failed");
    eprintln!("didOpen: OK ({:?})", wall.elapsed());

    // documentSymbol
    let symbols = client
        .document_symbols(&file_url, a_ts.to_str().unwrap())
        .await
        .expect("documentSymbol failed");
    eprintln!(
        "documentSymbol: OK ({:?}) — {} symbols",
        wall.elapsed(),
        symbols.len()
    );
    for sym in &symbols {
        eprintln!("  {:?} {} @ line {}", sym.kind, sym.name, sym.line);
    }

    // Assert greet is present
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"greet"),
        "expected 'greet' symbol, got: {names:?}"
    );

    // Shutdown
    client.shutdown().await.ok();

    let elapsed = wall.elapsed();
    eprintln!("Total wall-clock: {elapsed:?}");
    assert!(
        elapsed.as_secs() < 30,
        "test took too long: {elapsed:?} (limit 30 s)"
    );
}

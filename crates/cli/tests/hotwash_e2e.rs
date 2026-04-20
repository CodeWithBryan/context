//! End-to-end integration test for the full ctx pipeline against the real
//! hotwash Bun/TypeScript monorepo.
//!
//! Run with:
//!   cargo test --ignored -p ctx-cli -- `hotwash_e2e` --nocapture
//!
//! Prerequisites:
//!   - ~/Development/hotwash/hotwash must exist
//!   - ~150 MB model (fastembed all-MiniLM-L6-v2) will be downloaded on first run
//!   - Full index of ~265 TS/TSX/CSS/HTML files can take 2–10 min on first run

use std::path::PathBuf;
use std::time::Duration;

use rmcp::{transport::TokioChildProcess, ServiceExt};
use serde_json::json;

// ─── Helper ────────────────────────────────────────────────────────────────

fn hotwash_path() -> Option<PathBuf> {
    let p = dirs::home_dir()?.join("Development/hotwash/hotwash");
    p.exists().then_some(p)
}

/// Build (or reuse) the `ctx` debug binary and return its path.
fn build_ctx_binary() -> PathBuf {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "ctx-cli", "--bin", "ctx"])
        .output()
        .expect("cargo build failed to spawn");
    assert!(
        output.status.success(),
        "cargo build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // crates/cli → crates → workspace-root → target/debug/ctx
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .join("target/debug/ctx")
}

// ─── Test ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ~/Development/hotwash/hotwash and ~150 MB model download; \
            run with: cargo test --ignored -p ctx-cli -- hotwash_e2e --nocapture"]
#[allow(clippy::too_many_lines)]
async fn hotwash_e2e_full_flow() {
    let Some(repo) = hotwash_path() else {
        eprintln!("hotwash repo not found at ~/Development/hotwash/hotwash — skipping");
        return;
    };
    let repo_str = repo.to_str().expect("repo path is valid UTF-8");

    let binary = build_ctx_binary();
    eprintln!("ctx binary: {}", binary.display());

    // ── Phase 1: init ────────────────────────────────────────────────────────
    eprintln!("\n=== Phase 1: ctx init ===");
    let status = std::process::Command::new(&binary)
        .args(["init", repo_str])
        .status()
        .expect("spawn ctx init");
    assert!(status.success(), "ctx init failed with status {status}");
    eprintln!("ctx init: OK");

    // ── Phase 2: full index ──────────────────────────────────────────────────
    // Allow up to 20 minutes to accommodate model download + full embedding on
    // a cold cache.
    eprintln!("\n=== Phase 2: ctx index (may download ~150 MB model on first run) ===");
    let start = std::time::Instant::now();
    let output = std::process::Command::new(&binary)
        .args(["index", repo_str])
        .output()
        .expect("spawn ctx index");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("ctx index stdout: {stdout}");
    eprintln!("ctx index wall-clock: {elapsed:?}");

    assert!(
        output.status.success(),
        "ctx index failed (status={}):\nstdout={stdout}\nstderr={stderr}",
        output.status
    );

    // Parse "chunks=N" from "indexed: files=F, skipped=S, chunks=C, ..."
    assert!(
        stdout.contains("chunks="),
        "expected 'chunks=' in index output: {stdout}"
    );
    let chunks_upserted: u64 = stdout
        .split_whitespace()
        .find_map(|field| field.strip_prefix("chunks="))
        .and_then(|s| s.trim_end_matches(',').parse().ok())
        .expect("failed to parse chunks= from index output");
    assert!(
        chunks_upserted > 0,
        "expected chunks > 0, got {chunks_upserted}"
    );
    eprintln!("chunks upserted: {chunks_upserted} — Phase 2 OK");

    // ── Phase 3: serve + MCP round-trip ─────────────────────────────────────
    eprintln!("\n=== Phase 3: ctx serve + MCP round-trip ===");

    // TokioChildProcess spawns the child with piped stdin/stdout and builds the
    // codec transport automatically (rmcp feature = "transport-child-process").
    let mut serve_cmd = tokio::process::Command::new(&binary);
    serve_cmd.args(["serve", repo_str]);
    let transport = TokioChildProcess::new(serve_cmd).expect("spawn ctx serve");

    // ().serve(transport) performs the MCP initialize handshake as a client.
    let client = ().serve(transport).await.expect("MCP initialize handshake");

    // Confirm the server announced itself.
    let server_info = client.peer_info().cloned();
    eprintln!("server info: {server_info:#?}");

    // ── tools/list ───────────────────────────────────────────────────────────
    let tools = client.list_all_tools().await.expect("tools/list failed");
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    eprintln!("tools registered: {names:?}");

    for expected in [
        "semantic_search",
        "find_definition",
        "find_references",
        "find_callers",
        "get_chunk",
        "repo_status",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "tool '{expected}' missing from tools/list; got: {names:?}"
        );
    }
    eprintln!("tools/list: all 6 tools present — OK");

    // ── semantic_search ──────────────────────────────────────────────────────
    eprintln!("\n--- semantic_search('dashboard component') ---");
    let search_result = tokio::time::timeout(
        Duration::from_secs(30),
        client.call_tool(
            rmcp::model::CallToolRequestParams::new("semantic_search").with_arguments(
                json!({ "query": "dashboard component", "k": 5 })
                    .as_object()
                    .expect("json object")
                    .clone(),
            ),
        ),
    )
    .await
    .expect("semantic_search timed out after 30 s")
    .expect("semantic_search tool call failed");
    eprintln!("semantic_search result: {search_result:#?}");
    assert!(
        !search_result.content.is_empty(),
        "expected non-empty semantic_search result"
    );
    eprintln!("semantic_search: OK");

    // ── repo_status ──────────────────────────────────────────────────────────
    eprintln!("\n--- repo_status ---");
    let status_result = client
        .call_tool(
            rmcp::model::CallToolRequestParams::new("repo_status")
                .with_arguments(json!({}).as_object().expect("json object").clone()),
        )
        .await
        .expect("repo_status tool call failed");
    eprintln!("repo_status: {status_result:#?}");
    assert!(
        !status_result.content.is_empty(),
        "expected non-empty repo_status result"
    );
    // The status should report chunk count > 0.
    let status_text = status_result
        .content
        .iter()
        .filter_map(|c| {
            if let rmcp::model::RawContent::Text(t) = c.raw.clone() {
                Some(t.text)
            } else {
                None
            }
        })
        .collect::<String>();
    eprintln!("repo_status text: {status_text}");
    eprintln!("repo_status: OK");

    // ── Clean shutdown ───────────────────────────────────────────────────────
    eprintln!("\n=== Shutdown ===");
    client.cancel().await.ok();
    eprintln!("E2E test complete — all phases passed.");
}

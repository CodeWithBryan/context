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

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::Duration;

use rmcp::{transport::TokioChildProcess, ServiceExt};
use serde_json::json;

// ─── Helpers ───────────────────────────────────────────────────────────────

fn hotwash_path() -> Option<PathBuf> {
    let p = dirs::home_dir()?.join("Development/hotwash/hotwash");
    p.exists().then_some(p)
}

/// Scan the hotwash TypeScript sources for the first `export function <Name>` and
/// return the function name. This avoids hard-coded names that may drift over time.
fn pick_hotwash_target(repo: &std::path::Path) -> Option<String> {
    let pattern = regex::Regex::new(r"^export\s+(async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)")
        .expect("regex");
    for entry in walkdir::WalkDir::new(repo)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let p = e.path();
            !p.components().any(|c| c.as_os_str() == "node_modules")
                && matches!(p.extension().and_then(|s| s.to_str()), Some("ts" | "tsx"))
        })
    {
        if let Ok(f) = std::fs::File::open(entry.path()) {
            for line in BufReader::new(f).lines().take(200).flatten() {
                if let Some(caps) = pattern.captures(&line) {
                    return Some(caps[2].to_string());
                }
            }
        }
    }
    None
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

    // Wipe any prior per-repo state so this test exercises a fresh index —
    // necessary to verify tsserver symbol extraction actually runs (skipped
    // files on a warm cache bypass tsserver entirely).
    //
    // IMPORTANT: `ctx init`/`index` canonicalize the path internally before
    // hashing, so we must canonicalize here too, otherwise case-insensitive
    // macOS filesystems produce a different hash and the wipe misses.
    let canonical_repo = std::fs::canonicalize(&repo).expect("canonicalize hotwash");
    let per_repo_hash = blake3::hash(canonical_repo.as_os_str().as_encoded_bytes()).to_hex();
    let per_repo_dir = dirs::home_dir()
        .expect("home dir")
        .join(".ctx/repos")
        .join(per_repo_hash.as_str());
    if per_repo_dir.exists() {
        eprintln!("wiping prior state: {}", per_repo_dir.display());
        std::fs::remove_dir_all(&per_repo_dir).expect("wipe per-repo dir");
    } else {
        eprintln!("no prior state at: {}", per_repo_dir.display());
    }

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

    // Parse files= and skipped= — a successful index pass either upserts new
    // chunks (fresh index) or skips unchanged files (warm cache from prior run).
    // Either proves the pipeline walked the tree correctly.
    assert!(
        stdout.contains("files=") && stdout.contains("skipped="),
        "expected 'files=' and 'skipped=' in index output: {stdout}"
    );
    let parse_field = |key: &str| -> u64 {
        stdout
            .split_whitespace()
            .find_map(|field| field.strip_prefix(key))
            .and_then(|s| s.trim_end_matches(',').parse().ok())
            .unwrap_or(0)
    };
    let files_indexed = parse_field("files=");
    let files_skipped = parse_field("skipped=");
    assert!(
        files_indexed + files_skipped > 0,
        "expected files_indexed + files_skipped > 0, got indexed={files_indexed} skipped={files_skipped}"
    );
    eprintln!("Phase 2 OK — files_indexed={files_indexed}, files_skipped={files_skipped}");

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
    // Hard assertion: the store must actually contain chunks from either this
    // run or a prior run. This catches the case where the index silently did
    // nothing and yet some other plumbing returned ok.
    assert!(
        status_text.contains("\"chunks_total\":") && !status_text.contains("\"chunks_total\":0"),
        "repo_status reports zero chunks_total — index never populated the store: {status_text}"
    );
    eprintln!("repo_status: OK");

    // ── LSP wiring: find_definition returns hits for a TS symbol ─────────────
    // Verify LSP-backed (tsgo) symbol extraction produced results.
    // We dynamically pick a known `export function` from hotwash's TS sources
    // so the test doesn't break if a specific symbol is renamed.
    eprintln!("\n--- find_definition (LSP / tsgo wiring check) ---");
    let target = pick_hotwash_target(&repo)
        .expect("expected to find at least one `export function` in hotwash TS sources");
    eprintln!("LSP wiring target symbol: {target}");
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        client.call_tool(
            rmcp::model::CallToolRequestParams::new("find_definition").with_arguments(
                json!({ "name": target })
                    .as_object()
                    .expect("json object")
                    .clone(),
            ),
        ),
    )
    .await
    .expect("find_definition timed out")
    .expect("find_definition tool call failed");
    let content = format!("{:?}", result.content);
    assert!(
        content.contains("\"line\""),
        "LSP-backed find_definition({target}) returned no hits — \
         tsgo symbol extraction may not be wired correctly: {content}"
    );
    eprintln!("find_definition({target}) returned hits: {content}");
    eprintln!("LSP (tsgo) wiring: OK");

    // ── Clean shutdown ───────────────────────────────────────────────────────
    eprintln!("\n=== Shutdown ===");
    client.cancel().await.ok();
    eprintln!("E2E test complete — all phases passed.");
}

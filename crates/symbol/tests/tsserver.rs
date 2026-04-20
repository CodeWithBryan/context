use ctx_symbol::tsserver::TsServer;
use std::path::PathBuf;

#[tokio::test]
#[ignore = "spawns Node tsserver; ensure `node` + `tsserver` are available"]
async fn navtree_returns_function_symbols() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let fixture_file = fixture_dir.join("sample.ts");
    // Write a fixture on the fly so we don't depend on ctx-parse
    std::fs::create_dir_all(&fixture_dir).ok();
    std::fs::write(
        &fixture_file,
        "export function hello(): string { return 'hi'; }\n",
    )
    .unwrap();

    let server = TsServer::spawn(&fixture_dir).expect("spawn tsserver");
    let symbols = server.navtree(&fixture_file).await.expect("navtree");
    assert!(
        symbols.iter().any(|s| s.name == "hello"),
        "expected 'hello' in navtree, got {symbols:?}"
    );
    server.shutdown().await.ok();
}

#[test]
fn tsserver_graceful_degradation_when_missing() {
    // If tsserver binary is not present at the resolved path, spawn should
    // return a specific error (not panic). We don't have a way to guarantee
    // absence, but at least verify the function resolves and doesn't panic
    // unexpectedly. This runs by default (no #[ignore]).
    let tmp = std::env::temp_dir().join(format!("ctx-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok();
    // No node_modules, no tsserver — spawn should either succeed (if global tsserver) or fail gracefully.
    let result = TsServer::spawn(&tmp);
    // Either way, should not panic. Success or clean error.
    let _ = result;
}

/// Fix 7: verify that `try_spawn` returns `Ok(None)` — not `Err` — when tsserver
/// is absent, confirming graceful degradation.
#[tokio::test]
async fn try_spawn_returns_none_when_tsserver_missing() {
    let tmp = std::env::temp_dir().join(format!("ctx-test-none-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok();
    // Clear any env override so we truly test the "nothing found" path.
    let _guard = TempEnv::remove("CTX_TSSERVER_PATH");
    let res = TsServer::try_spawn(&tmp).await;
    match res {
        Ok(None) => {} // expected: graceful degradation
        Ok(Some(_)) => panic!("expected None when tsserver not present"),
        Err(e) => panic!("try_spawn should not error on missing tsserver, got: {e:?}"),
    }
}

/// Minimal helper to scope env-var changes within a test without an
/// external crate.
struct TempEnv {
    key: &'static str,
    prev: Option<String>,
}

impl TempEnv {
    fn remove(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for TempEnv {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            std::env::set_var(self.key, prev);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

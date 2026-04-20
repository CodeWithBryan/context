use ctx_watch::Watcher;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn watcher_emits_batched_events_for_rapid_edits() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("a.ts"), "export const a = 1;\n").unwrap();
    std::fs::write(root.join("b.ts"), "export const b = 2;\n").unwrap();

    let mut handle = Watcher::start(&root, Duration::from_millis(200)).unwrap();

    // Give the watcher a moment to register
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Trigger two rapid edits; expect a single debounced batch
    std::fs::write(root.join("a.ts"), "export const a = 11;\n").unwrap();
    std::fs::write(root.join("b.ts"), "export const b = 22;\n").unwrap();

    let batch = tokio::time::timeout(Duration::from_secs(2), handle.rx.recv())
        .await
        .expect("batch timeout")
        .expect("receiver closed");

    assert!(
        !batch.is_empty() && batch.len() <= 2,
        "expected 1-2 paths in batch, got {}",
        batch.len()
    );
    let names: Vec<_> = batch
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str().map(String::from)))
        .collect();
    assert!(
        names.iter().any(|n| n == "a.ts" || n == "b.ts"),
        "expected a.ts or b.ts in batch, got: {names:?}"
    );
}

#[tokio::test]
async fn watcher_filters_node_modules_and_dotdirs() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();

    let mut handle = Watcher::start(&root, Duration::from_millis(150)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Write files in all three locations
    std::fs::write(root.join("node_modules/pkg/index.ts"), "x").unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    std::fs::write(root.join("src/app.ts"), "export const app = 1;").unwrap();

    // Collect batches for ~400ms to give everything a chance
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.rx.recv()).await {
            Ok(Some(batch)) => seen.extend(batch),
            _ => break,
        }
    }

    // src/app.ts should appear; node_modules and .git paths must NOT
    assert!(
        seen.iter().any(|p| p.ends_with("src/app.ts")),
        "expected src/app.ts in events, got: {seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|p| p.components().any(|c| c.as_os_str() == "node_modules")),
        "node_modules path leaked: {seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|p| p.components().any(|c| c.as_os_str() == ".git")),
        ".git path leaked: {seen:?}"
    );
}

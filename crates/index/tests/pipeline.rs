use async_trait::async_trait;
use ctx_core::traits::{ChunkStore, Embedder, RefStore};
use ctx_core::Result;
use ctx_core::Scope;
use ctx_index::{IndexReport, Pipeline};
use ctx_store::{LanceChunkStore, RedbRefStore};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

/// Deterministic embedder for tests — vector = [byte-sum-mod-256 as f32, 0, 0, 0]
struct MockEmbedder {
    dim: usize,
}

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let sum: u32 = t.bytes().map(u32::from).sum();
            let mut v = vec![0.0_f32; self.dim];
            #[allow(clippy::cast_precision_loss)]
            let val = (sum % 256) as f32;
            v[0] = val;
            out.push(v);
        }
        Ok(out)
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn model_id(&self) -> &'static str {
        "mock"
    }
}

fn copy_fixtures(dest: &Path) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minirepo");
    let dest_mini = dest.join("minirepo");
    std::fs::create_dir_all(&dest_mini).unwrap();
    for entry in std::fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        std::fs::copy(entry.path(), dest_mini.join(&name)).unwrap();
    }
    dest_mini
}

async fn new_pipeline(
    root: &Path,
    dim: usize,
) -> (Pipeline<LanceChunkStore, RedbRefStore, MockEmbedder>, Scope) {
    let chunks = LanceChunkStore::open(root.join("lance"), dim).await.unwrap();
    let refs = RedbRefStore::open(root.join("refs.redb")).unwrap();
    let embed = MockEmbedder { dim };
    let pipeline = Pipeline::new(chunks, refs, embed);
    let src_root = root.join("minirepo");
    let scope = Scope::local(&src_root, &src_root, Some("main".into())).unwrap();
    (pipeline, scope)
}

#[tokio::test]
async fn full_index_produces_chunks_and_refs() {
    let dir = tempdir().unwrap();
    let src_root = copy_fixtures(dir.path());
    let (pipeline, scope) = new_pipeline(dir.path(), 4).await;

    let report: IndexReport = pipeline.full_index(&scope, &src_root).await.unwrap();
    assert!(
        report.files_indexed >= 3,
        "expected 3+ files indexed, got {}",
        report.files_indexed
    );
    assert!(
        report.chunks_upserted > 0,
        "expected >0 chunks upserted, got {}",
        report.chunks_upserted
    );

    assert!(pipeline.chunks().count().await.unwrap() > 0);
    let active = pipeline.refs().active_hashes(&scope).await.unwrap();
    assert!(!active.is_empty(), "expected active hashes");
}

#[tokio::test]
async fn incremental_index_skips_unchanged_files() {
    let dir = tempdir().unwrap();
    let src_root = copy_fixtures(dir.path());
    let (pipeline, scope) = new_pipeline(dir.path(), 4).await;

    let first = pipeline.full_index(&scope, &src_root).await.unwrap();
    let first_files = first.files_indexed;
    let first_embeds = first.chunks_embedded;

    // No changes → all files skipped, zero re-embeds on second run
    let second = pipeline.full_index(&scope, &src_root).await.unwrap();
    assert_eq!(
        second.files_indexed, 0,
        "second pass should index 0 files (all skipped), got {}",
        second.files_indexed
    );
    assert_eq!(
        second.files_skipped, first_files,
        "second pass should skip all {} files, got {}",
        first_files, second.files_skipped
    );
    assert_eq!(
        second.chunks_embedded, 0,
        "expected zero re-embeds, got {}",
        second.chunks_embedded
    );
    // first_embeds was > 0 on first run
    assert!(first_embeds > 0);
}

#[tokio::test]
async fn modifying_one_file_reindexes_only_that_file() {
    let dir = tempdir().unwrap();
    let src_root = copy_fixtures(dir.path());
    let (pipeline, scope) = new_pipeline(dir.path(), 4).await;

    pipeline.full_index(&scope, &src_root).await.unwrap();

    // Modify a.ts
    let modified = src_root.join("a.ts");
    std::fs::write(
        &modified,
        "export function greet(n: string): string { return n; }\nexport function extra(): number { return 42; }\n",
    )
    .unwrap();

    let incremental = pipeline
        .incremental(&scope, &src_root, std::slice::from_ref(&modified))
        .await
        .unwrap();
    // Only 1 file processed
    assert_eq!(incremental.files_indexed, 1);
    // New content should produce some new chunks that need embedding
    assert!(
        incremental.chunks_embedded >= 1,
        "expected >=1 new embed, got {}",
        incremental.chunks_embedded
    );
}

#[tokio::test]
async fn shrinking_file_removes_stale_refs() {
    use ctx_core::traits::RefStore;
    let dir = tempdir().unwrap();
    let src_root = copy_fixtures(dir.path());
    let (pipeline, scope) = new_pipeline(dir.path(), 4).await;

    pipeline.full_index(&scope, &src_root).await.unwrap();
    let active_before = pipeline.refs().active_hashes(&scope).await.unwrap();
    let count_before = active_before.len();

    // Shrink a.ts: replace the entire file with a single trivial line, which
    // will produce far fewer chunks than the original.
    let file = src_root.join("a.ts");
    std::fs::write(&file, "export const x = 1;\n").unwrap();

    pipeline
        .incremental(&scope, &src_root, std::slice::from_ref(&file))
        .await
        .unwrap();
    let active_after = pipeline.refs().active_hashes(&scope).await.unwrap();

    assert!(
        active_after.len() < count_before,
        "expected fewer active hashes after shrink (before={count_before}, after={})",
        active_after.len()
    );
}

#[tokio::test]
async fn unchanged_files_count_as_skipped_not_indexed() {
    let dir = tempdir().unwrap();
    let src_root = copy_fixtures(dir.path());
    let (pipeline, scope) = new_pipeline(dir.path(), 4).await;

    pipeline.full_index(&scope, &src_root).await.unwrap();
    let second = pipeline.full_index(&scope, &src_root).await.unwrap();

    assert_eq!(second.files_indexed, 0, "second pass should index 0 files");
    assert!(
        second.files_skipped >= 3,
        "second pass should skip all 3 fixture files, got {}",
        second.files_skipped
    );
    assert_eq!(second.chunks_embedded, 0);
}

#[tokio::test]
async fn ignores_node_modules_and_dotfiles() {
    let dir = tempdir().unwrap();
    let src_root = copy_fixtures(dir.path());
    std::fs::create_dir_all(src_root.join("node_modules/pkg")).unwrap();
    std::fs::write(
        src_root.join("node_modules/pkg/index.ts"),
        "export const x = 1;",
    )
    .unwrap();
    std::fs::write(src_root.join(".hidden.ts"), "export const h = 1;").unwrap();

    let (pipeline, scope) = new_pipeline(dir.path(), 4).await;
    let report = pipeline.full_index(&scope, &src_root).await.unwrap();

    // Only the 3 original files — node_modules and .hidden skipped
    assert_eq!(report.files_indexed, 3);
}

//! Indexing pipeline — orchestrates parse, embed, store, and symbol extraction.

use ctx_core::traits::{ChunkStore, Embedder, RefStore};
use ctx_core::{ChunkRef, ContentHash, CtxError, Result, Scope};
use ctx_symbol::lsp::LspClient;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Summary of one indexing run.
#[derive(Debug, Default, Clone)]
pub struct IndexReport {
    pub files_indexed: u64,
    pub files_skipped: u64,
    pub chunks_upserted: u64,
    pub chunks_embedded: u64,
    pub symbols_upserted: u64,
    pub errors: u64,
}

/// Orchestrates parse → embed → store → symbol extraction for a set of files.
pub struct Pipeline<C: ChunkStore, R: RefStore, E: Embedder> {
    chunks: Arc<C>,
    refs: Arc<R>,
    embedder: Arc<E>,
    ts_lsp: Option<Arc<LspClient>>,
}

impl<C: ChunkStore, R: RefStore, E: Embedder> Pipeline<C, R, E> {
    #[must_use]
    pub fn new(chunks: C, refs: R, embedder: E) -> Self {
        Self {
            chunks: Arc::new(chunks),
            refs: Arc::new(refs),
            embedder: Arc::new(embedder),
            ts_lsp: None,
        }
    }

    /// Construct a pipeline from pre-existing `Arc`s, allowing the same
    /// store/embedder instances to be shared with a `Router` in `serve`.
    #[must_use]
    pub fn new_shared(chunks: Arc<C>, refs: Arc<R>, embedder: Arc<E>) -> Self {
        Self {
            chunks,
            refs,
            embedder,
            ts_lsp: None,
        }
    }

    /// Attach an LSP client for TS/JS structural symbol extraction. Idempotent.
    /// Pass `None` to explicitly skip LSP (e.g. environments without tsgo).
    #[must_use]
    pub fn with_ts_lsp(mut self, lsp: Option<Arc<LspClient>>) -> Self {
        self.ts_lsp = lsp;
        self
    }

    /// Return a clone of the inner `LspClient` Arc, if one is attached.
    #[must_use]
    pub fn ts_lsp_ref(&self) -> Option<Arc<LspClient>> {
        self.ts_lsp.clone()
    }

    #[must_use]
    pub fn chunks(&self) -> Arc<C> {
        self.chunks.clone()
    }

    #[must_use]
    pub fn refs(&self) -> Arc<R> {
        self.refs.clone()
    }

    /// Walk `root` (honouring `.gitignore`, hidden files, and `node_modules`)
    /// and index every supported file.
    pub async fn full_index(&self, scope: &Scope, root: &Path) -> Result<IndexReport> {
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true) // skip dotfiles / hidden dirs
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            // The `ignore` crate does NOT skip node_modules by default unless a
            // .gitignore covers it. We add a custom filter here so tests pass
            // even without a .gitignore in the fixture dir.
            .filter_entry(|e| e.file_name().to_str() != Some("node_modules"))
            .build();

        let mut report = IndexReport::default();
        for entry in walker.filter_map(std::result::Result::ok) {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            if ctx_parse::detect(path).is_none() {
                continue;
            }
            match self.index_file(scope, path).await {
                Ok(r) => {
                    if r.skipped {
                        report.files_skipped += 1;
                    } else {
                        report.files_indexed += 1;
                        report.chunks_upserted += r.chunks_upserted;
                        report.chunks_embedded += r.chunks_embedded;
                        report.symbols_upserted += r.symbols_upserted;
                    }
                }
                Err(e) => {
                    tracing::warn!("index {}: {e}", path.display());
                    report.errors += 1;
                }
            }
        }
        Ok(report)
    }

    /// Index only the explicitly provided `changed` paths (used by the file-watcher).
    pub async fn incremental(
        &self,
        scope: &Scope,
        root: &Path,
        changed: &[PathBuf],
    ) -> Result<IndexReport> {
        let mut report = IndexReport::default();
        for path in changed {
            if !path.exists() {
                // Deleted files are handled elsewhere for Phase 1.
                continue;
            }
            if ctx_parse::detect(path).is_none() {
                continue;
            }
            if !path.starts_with(root) {
                continue;
            }
            match self.index_file(scope, path).await {
                Ok(r) => {
                    if r.skipped {
                        report.files_skipped += 1;
                    } else {
                        report.files_indexed += 1;
                        report.chunks_upserted += r.chunks_upserted;
                        report.chunks_embedded += r.chunks_embedded;
                        report.symbols_upserted += r.symbols_upserted;
                    }
                }
                Err(e) => {
                    tracing::warn!("incremental {}: {e}", path.display());
                    report.errors += 1;
                }
            }
        }
        Ok(report)
    }

    /// Index a single file: read → hash-dedup → chunk → embed-dedup → store → symbol.
    #[allow(clippy::too_many_lines)]
    async fn index_file(&self, scope: &Scope, path: &Path) -> Result<FileReport> {
        let bytes = tokio::fs::read(path).await?;
        let file_hash = ContentHash::of(&bytes);
        let file_str = path.to_string_lossy().to_string();

        // Skip if the file content hash is unchanged since last index.
        if let Ok(Some(prev)) = self.refs.file_hash(scope, &file_str).await {
            if prev == file_hash {
                return Ok(FileReport {
                    skipped: true,
                    ..Default::default()
                });
            }
        }

        // Re-indexing: wipe stale refs + symbols for this file FIRST so that
        // shrinking a file doesn't leave orphaned chunk refs in the active set,
        // and repeated indexing doesn't duplicate symbols.
        self.refs.clear_file_state(scope, &file_str).await?;

        let chunks = ctx_parse::Chunker::new().chunk(&file_str, &bytes)?;
        if chunks.is_empty() {
            // Record the hash so we don't re-chunk on every pass.
            self.refs
                .record_file_hash(scope, &file_str, file_hash)
                .await?;
            return Ok(FileReport::default());
        }

        // Per-chunk dedup: only embed chunks not already stored.
        // TODO(perf): this does N async calls to ChunkStore::get for a file with N chunks.
        // Add `ChunkStore::contains_hashes(&[ContentHash]) -> Result<HashSet<ContentHash>>`
        // to the trait and use it here for a single-call batch check.
        let mut to_embed: Vec<usize> = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            let exists = self.chunks.get(&c.hash).await?.is_some();
            if !exists {
                to_embed.push(i);
            }
        }

        let mut report = FileReport::default();

        if !to_embed.is_empty() {
            let texts: Vec<&str> = to_embed.iter().map(|&i| chunks[i].text.as_str()).collect();
            let vectors = self.embedder.embed(&texts).await?;
            if vectors.len() != to_embed.len() {
                return Err(CtxError::Embed(format!(
                    "embedder returned {} vectors for {} texts",
                    vectors.len(),
                    to_embed.len()
                )));
            }
            let mut new_chunks = Vec::with_capacity(to_embed.len());
            for (idx, v) in to_embed.iter().zip(vectors) {
                let mut c = chunks[*idx].clone();
                c.vector = Some(v);
                new_chunks.push(c);
            }
            self.chunks.upsert(&new_chunks).await?;
            report.chunks_embedded = u64::try_from(new_chunks.len()).unwrap_or(u64::MAX);
            report.chunks_upserted = report.chunks_embedded;
        }

        // Bind refs: replace all refs for this file with the current chunk set.
        let refs: Vec<ChunkRef> = chunks
            .iter()
            .map(|c| ChunkRef {
                hash: c.hash,
                file: file_str.clone(),
                line_range: c.line_range,
            })
            .collect();
        self.refs.bind(scope, &refs).await?;

        // Extract symbols: tree-sitter for CSS/HTML; LSP documentSymbol for TS/JS.
        let mut symbols = ctx_symbol::extractor::extract_from_file(&file_str, &bytes)?;

        let lang = ctx_parse::detect(path);
        if matches!(
            lang,
            Some(
                ctx_core::Language::TypeScript
                    | ctx_core::Language::Tsx
                    | ctx_core::Language::JavaScript
                    | ctx_core::Language::Jsx
            )
        ) {
            if let Some(lsp) = &self.ts_lsp {
                let uri = url::Url::from_file_path(path).map_err(|()| {
                    CtxError::Symbol(format!("not a valid file URL: {}", path.display()))
                })?;
                let language_id = match lang.expect("matched above") {
                    ctx_core::Language::TypeScript => "typescript",
                    ctx_core::Language::Tsx => "typescriptreact",
                    ctx_core::Language::JavaScript => "javascript",
                    ctx_core::Language::Jsx => "javascriptreact",
                    _ => unreachable!(),
                };
                let text = std::str::from_utf8(&bytes).unwrap_or("");
                if let Err(e) = lsp.did_open(&uri, language_id, text).await {
                    tracing::warn!("LSP didOpen {}: {e}", path.display());
                }
                match lsp.document_symbols(&uri, &file_str).await {
                    Ok(mut ts_symbols) => symbols.append(&mut ts_symbols),
                    Err(e) => {
                        tracing::warn!("LSP documentSymbol {}: {e}", path.display());
                        // Do NOT fail the file — LSP errors are recoverable.
                    }
                }
            }
        }

        if !symbols.is_empty() {
            let count = u64::try_from(symbols.len()).unwrap_or(u64::MAX);
            self.refs.upsert_symbols(scope, &symbols).await?;
            report.symbols_upserted = count;
        }

        self.refs
            .record_file_hash(scope, &file_str, file_hash)
            .await?;
        Ok(report)
    }
}

#[derive(Default)]
struct FileReport {
    skipped: bool,
    chunks_upserted: u64,
    chunks_embedded: u64,
    symbols_upserted: u64,
}

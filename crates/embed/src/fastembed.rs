use async_trait::async_trait;
use ctx_core::traits::Embedder;
use ctx_core::{CtxError, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

/// Runtime-tunable options for `FastembedEmbedder`. Use `EmbedderOptions::default()`
/// for the standard interactive-CLI defaults.
#[derive(Clone, Debug)]
pub struct EmbedderOptions {
    /// Emit fastembed's `indicatif` progress bar during first-run download.
    /// Default: `true`. Set to `false` for MCP/stdio or any quiet context.
    pub show_download_progress: bool,
    /// Optional override for the model cache directory.
    /// Default: `dirs::cache_dir()/ctx/fastembed/`.
    pub cache_dir: Option<PathBuf>,
}

impl Default for EmbedderOptions {
    fn default() -> Self {
        Self {
            show_download_progress: true,
            cache_dir: None,
        }
    }
}

/// Local embedder backed by fastembed-rs. Defaults to `nomic-embed-text-v1.5`
/// (768-dim, ~150 MB ONNX model). The model is downloaded from Hugging Face
/// on first use and cached in the user-level cache directory.
pub struct FastembedEmbedder {
    inner: Arc<Mutex<TextEmbedding>>,
    dim: usize,
    model_id: String,
}

impl FastembedEmbedder {
    /// Initialise with the default code-capable embedding model and show download
    /// progress. Use from interactive CLI contexts.
    pub async fn new_default() -> Result<Self> {
        Self::new_with_options(EmbedderOptions::default()).await
    }

    /// Initialise silently — suitable for MCP stdio servers or any context where
    /// stdout/stderr must remain clean.
    pub async fn new_silent() -> Result<Self> {
        Self::new_with_options(EmbedderOptions {
            show_download_progress: false,
            ..Default::default()
        })
        .await
    }

    /// Initialise with a fully-specified options struct.
    pub async fn new_with_options(opts: EmbedderOptions) -> Result<Self> {
        let model = EmbeddingModel::NomicEmbedTextV15;

        let cache_dir = opts.cache_dir.clone().unwrap_or_else(|| {
            let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".fastembed_cache"));
            base.join("ctx").join("fastembed")
        });

        let te = tokio::task::spawn_blocking(move || {
            TextEmbedding::try_new(
                TextInitOptions::new(model)
                    .with_show_download_progress(opts.show_download_progress)
                    .with_cache_dir(cache_dir),
            )
        })
        .await
        .map_err(|e| CtxError::Embed(format!("fastembed init join: {e}")))?
        .map_err(|e| CtxError::Embed(format!("fastembed init: {e}")))?;

        Ok(Self {
            inner: Arc::new(Mutex::new(te)),
            dim: 768,
            model_id: "nomic-embed-text-v1.5".to_string(),
        })
    }
}

#[async_trait]
impl Embedder for FastembedEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let te = self.inner.clone();
        // fastembed v5 requires owned strings via AsRef<str>; convert once at boundary.
        let owned: Vec<String> = texts.iter().map(|s| (*s).to_string()).collect();
        tokio::task::spawn_blocking(move || {
            // reason: the lock guard must be held across the embed call; tightening
            // the scope would require an unsafe transmute of the guard's lifetime.
            #[allow(clippy::significant_drop_tightening)]
            let mut guard = te.lock();
            guard.embed(owned, None)
        })
        .await
        .map_err(|e| CtxError::Embed(format!("fastembed embed join: {e}")))?
        .map_err(|e| CtxError::Embed(format!("fastembed embed: {e}")))
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

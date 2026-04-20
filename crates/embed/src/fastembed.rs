use async_trait::async_trait;
use ctx_core::traits::Embedder;
use ctx_core::{CtxError, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

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
/// (768-dim, ~150 MB ONNX model).
///
/// The underlying `TextEmbedding` is initialised **lazily** on first `embed()`
/// call. Constructing the struct is ~instant — ~2-4 seconds of ONNX model
/// load only happens when the first query lands. This matters a lot for
/// `ctx serve`: the MCP handshake becomes quick and the expensive init
/// only pays once, when the user actually needs a semantic search.
pub struct FastembedEmbedder {
    /// Initialised on the first `embed()` call. Wrapped in `Arc<Mutex<_>>`
    /// so the lock can be held inside `spawn_blocking` during inference.
    inner: OnceCell<Arc<Mutex<TextEmbedding>>>,
    opts: EmbedderOptions,
    dim: usize,
    model_id: String,
}

impl FastembedEmbedder {
    /// Create a fastembed embedder with interactive-CLI defaults (download
    /// progress bar enabled). Returns immediately — the ~150MB ONNX model
    /// is not loaded until the first `embed()` call.
    ///
    /// # Errors
    /// The constructor itself is infallible; errors surface lazily from the
    /// first call to `embed()`. The `Result` return is kept for API
    /// symmetry with earlier eager-init versions.
    pub async fn new_default() -> Result<Self> {
        Self::new_with_options(EmbedderOptions::default()).await
    }

    /// Create a silent embedder (suppresses fastembed's download progress
    /// bar). Use for MCP stdio servers where stdout/stderr must stay clean.
    ///
    /// Same lazy-init semantics as [`Self::new_default`].
    ///
    /// # Errors
    /// Infallible today; see [`Self::new_default`].
    pub async fn new_silent() -> Result<Self> {
        Self::new_with_options(EmbedderOptions {
            show_download_progress: false,
            ..Default::default()
        })
        .await
    }

    /// Create an embedder from a fully-specified options struct. Lazy init.
    ///
    /// # Errors
    /// Infallible today; see [`Self::new_default`].
    //
    // `async` is preserved for API compatibility with callers that already
    // `.await` this constructor. No await is needed now that init is lazy.
    #[allow(clippy::unused_async)]
    pub async fn new_with_options(opts: EmbedderOptions) -> Result<Self> {
        Ok(Self {
            inner: OnceCell::new(),
            opts,
            dim: 768,
            model_id: "nomic-embed-text-v1.5".to_string(),
        })
    }

    /// Resolve (and lazily load) the underlying `TextEmbedding`. Subsequent
    /// calls return the cached handle without reloading.
    async fn get_or_init(&self) -> Result<Arc<Mutex<TextEmbedding>>> {
        let arc = self
            .inner
            .get_or_try_init(|| async {
                let model = EmbeddingModel::NomicEmbedTextV15;
                let cache_dir = self.opts.cache_dir.clone().unwrap_or_else(|| {
                    let base =
                        dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".fastembed_cache"));
                    base.join("ctx").join("fastembed")
                });
                let show_progress = self.opts.show_download_progress;

                let te = tokio::task::spawn_blocking(move || {
                    TextEmbedding::try_new(
                        TextInitOptions::new(model)
                            .with_show_download_progress(show_progress)
                            .with_cache_dir(cache_dir),
                    )
                })
                .await
                .map_err(|e| CtxError::Embed(format!("fastembed init join: {e}")))?
                .map_err(|e| CtxError::Embed(format!("fastembed init: {e}")))?;

                Ok::<_, CtxError>(Arc::new(Mutex::new(te)))
            })
            .await?
            .clone();
        Ok(arc)
    }
}

#[async_trait]
impl Embedder for FastembedEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let te = self.get_or_init().await?;
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

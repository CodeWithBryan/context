//! ctx-watch — notify + debounce batched filesystem watcher. Implemented in Task 9.

pub mod debounce;

use ctx_core::{CtxError, Result};
use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use notify::RecommendedWatcher;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// A handle to a running file watcher.
///
/// Dropping this handle stops the underlying debouncer thread.
pub struct WatchHandle {
    /// Receiver for batches of changed file paths.
    pub rx: mpsc::Receiver<Vec<PathBuf>>,
    // Keep the debouncer alive for the lifetime of the handle.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// Thin wrapper around `notify-debouncer-full` that watches a directory
/// recursively and emits debounced batches of changed paths.
pub struct Watcher;

impl Watcher {
    /// Start watching `root` recursively with the given debounce window.
    ///
    /// Returns a [`WatchHandle`] whose `rx` field yields `Vec<PathBuf>` batches.
    /// Each batch contains deduplicated, filtered paths (junk dirs like
    /// `node_modules` and `.git` are excluded).
    ///
    /// # Errors
    /// Returns `CtxError::Other` if the underlying notify watcher cannot be
    /// created or if the root path cannot be watched.
    pub fn start(root: &Path, debounce: Duration) -> Result<WatchHandle> {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>(256);

        // notify-debouncer-full 0.7 callback: FnMut(DebounceEventResult) + Send + 'static
        let mut debouncer = new_debouncer(debounce, None, move |res: DebounceEventResult| {
            match res {
                Ok(events) => {
                    let mut paths: Vec<PathBuf> = Vec::new();
                    for ev in events {
                        for p in ev.event.paths {
                            paths.push(p);
                        }
                    }
                    let cleaned = debounce::clean_batch(paths);
                    if cleaned.is_empty() {
                        return;
                    }
                    // Best-effort send; drop batch if receiver is gone.
                    if let Err(e) = tx.blocking_send(cleaned) {
                        debug!("watcher receiver dropped: {e}");
                    }
                }
                Err(errs) => {
                    for e in errs {
                        warn!("notify error: {e:?}");
                    }
                }
            }
        })
        .map_err(|e| CtxError::Other(anyhow::anyhow!("new_debouncer: {e}")))?;

        debouncer
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| CtxError::Other(anyhow::anyhow!("watch: {e}")))?;

        Ok(WatchHandle {
            rx,
            _debouncer: debouncer,
        })
    }
}

//! ctx-index — indexing pipeline that orchestrates parse/embed/store/symbol. Implemented in Task 8.
//!
//! Per-file Merkle state lives in `RefStore::record_file_hash` / `file_hash`.
//! A future task could add a proper Merkle tree of directories for faster
//! "has any file changed?" queries. For Phase 1 the per-file hash is enough
//! because `incremental()` is only called with a specific file list from
//! the watcher (Task 9).

mod pipeline;

pub use pipeline::{IndexReport, Pipeline};

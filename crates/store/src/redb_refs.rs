// Key scheme for the ACTIVE table:
//   Tuple key: (scope_key: &str, chunk_hash: &[u8; 32])
//   redb 4 supports tuple keys natively via `impl Key for (T0, T1)`.
//   Range scans over all chunks for a given scope use (scope, [0x00;32])..=(scope, [0xff;32]).
//
// Key scheme for SYMBOLS_BY_NAME:
//   Tuple key: (scope_key: &str, symbol_name: &str)
//
// Key scheme for FILE_HASH:
//   Tuple key: (scope_key: &str, file_path: &str)
//   Value: &[u8; 32] (ContentHash bytes)
//
// Phase 1 deviations from plan:
//   - upsert_symbols appends without deduplication (per-Phase-1 spec).
//   - SymbolQuery::ByFile returns Err(CtxError::Unimplemented("ByFile")).
//   - redb 4 requires ReadableDatabase + ReadableTable traits in scope for
//     begin_read / get / range on Arc<Database> and Table respectively.
//   - Range bounds sentinel arrays are held in named locals (not inline
//     temporaries) to satisfy the borrow checker.

use async_trait::async_trait;
use ctx_core::traits::{RefStore, SymbolQuery};
use ctx_core::{ChunkRef, ContentHash, CtxError, Result, Scope, Symbol};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

// ACTIVE: maps (scope_key, chunk_hash_bytes) -> JSON-serialized ChunkRef
const ACTIVE: TableDefinition<(&str, &[u8; 32]), &[u8]> = TableDefinition::new("active");
// SYMBOLS_BY_NAME: maps (scope_key, symbol_name) -> JSON-serialized Vec<Symbol>
const SYMBOLS_BY_NAME: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("symbols_by_name");
// FILE_HASH: maps (scope_key, file_path) -> raw ContentHash bytes
const FILE_HASH: TableDefinition<(&str, &str), &[u8; 32]> = TableDefinition::new("file_hash");

/// Converts any `Display` error into `CtxError::Store`.
fn store_err(e: impl std::fmt::Display) -> CtxError {
    CtxError::Store(e.to_string())
}

pub struct RedbRefStore {
    db: Arc<Database>,
}

impl RedbRefStore {
    /// Open or create a redb database at `path`.
    ///
    /// # Errors
    /// Returns `CtxError::Store` if the database file cannot be created or is corrupt.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path).map_err(store_err)?;
        Ok(Self { db: Arc::new(db) })
    }

    fn scope_key(scope: &Scope) -> Result<String> {
        if scope.tenant.contains(':') {
            return Err(CtxError::Store(format!(
                "scope.tenant must not contain ':': {:?}", scope.tenant
            )));
        }
        if matches!(scope.branch.as_deref(), Some("_none")) {
            return Err(CtxError::Store(
                "branch name '_none' is reserved as the scope-key None sentinel".into()
            ));
        }
        Ok(format!(
            "{}:{}:{}:{}",
            scope.tenant,
            scope.repo.0.to_hex(),
            scope.worktree.0.to_hex(),
            scope.branch.as_deref().unwrap_or("_none"),
        ))
    }
}

#[async_trait]
impl RefStore for RedbRefStore {
    async fn bind(&self, scope: &Scope, refs: &[ChunkRef]) -> Result<()> {
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        let refs = refs.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let write = db.begin_write().map_err(store_err)?;
            {
                let mut active = write.open_table(ACTIVE).map_err(store_err)?;
                for r in &refs {
                    let bytes = serde_json::to_vec(r).map_err(store_err)?;
                    active
                        .insert((key.as_str(), &r.hash.0), bytes.as_slice())
                        .map_err(store_err)?;
                }
            }
            write.commit().map_err(store_err)
        })
        .await
        .map_err(|e| CtxError::Store(format!("bind join: {e}")))??;
        Ok(())
    }

    async fn active_hashes(&self, scope: &Scope) -> Result<HashSet<ContentHash>> {
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<HashSet<ContentHash>> {
            let read = db.begin_read().map_err(store_err)?;
            // open_table returns TableError::TableDoesNotExist if never written; treat as empty.
            let table = match read.open_table(ACTIVE) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(HashSet::new()),
                Err(e) => return Err(store_err(e)),
            };
            let mut out = HashSet::new();
            // Safety: redb encodes tuple keys with per-element length prefixes, so
            // (scope_a, [0xff; 32]) < (scope_a_longer, [0x00; 32]). The fixed-width
            // 32-byte hash means these sentinel bounds are exhaustive within exactly
            // this scope_key — they cannot spill into neighboring scopes. If the
            // ACTIVE key type is ever flattened to raw &[u8], this invariant breaks.
            // Hold the sentinel arrays in named locals so their references live long enough.
            let low = [0x00u8; 32];
            let high = [0xffu8; 32];
            let range_start = (key.as_str(), &low);
            let range_end = (key.as_str(), &high);
            for row in table.range(range_start..=range_end).map_err(store_err)? {
                let (k, _v) = row.map_err(store_err)?;
                let (_scope_str, hash_bytes) = k.value();
                out.insert(ContentHash(*hash_bytes));
            }
            Ok(out)
        })
        .await
        .map_err(|e| CtxError::Store(format!("active_hashes join: {e}")))?
    }

    async fn upsert_symbols(&self, scope: &Scope, symbols: &[Symbol]) -> Result<()> {
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        let syms = symbols.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            use std::collections::HashMap;
            let write = db.begin_write().map_err(store_err)?;
            {
                let mut table = write.open_table(SYMBOLS_BY_NAME).map_err(store_err)?;
                // Group by name (accumulate, not dedup — Phase 1 intentionally keeps duplicates).
                let mut grouped: HashMap<String, Vec<Symbol>> = HashMap::new();
                for s in syms {
                    grouped.entry(s.name.clone()).or_default().push(s);
                }
                for (name, mut new_syms) in grouped {
                    // Read existing symbols, copy the bytes out immediately so
                    // the AccessGuard (which borrows `table` immutably) is dropped
                    // before the mutable borrow in `table.insert(...)` below.
                    let existing_bytes: Option<Vec<u8>> = table
                        .get((key.as_str(), name.as_str()))
                        .map_err(store_err)?
                        .map(|guard| guard.value().to_vec());
                    if let Some(raw) = existing_bytes {
                        let mut prior: Vec<Symbol> =
                            serde_json::from_slice(&raw).map_err(store_err)?;
                        prior.append(&mut new_syms);
                        new_syms = prior;
                    }
                    let bytes = serde_json::to_vec(&new_syms).map_err(store_err)?;
                    table
                        .insert((key.as_str(), name.as_str()), bytes.as_slice())
                        .map_err(store_err)?;
                }
            }
            write.commit().map_err(store_err)
        })
        .await
        .map_err(|e| CtxError::Store(format!("upsert_symbols join: {e}")))??;
        Ok(())
    }

    async fn symbols(&self, scope: &Scope, q: SymbolQuery) -> Result<Vec<Symbol>> {
        let name = match q {
            SymbolQuery::Definition { name }
            | SymbolQuery::References { name }
            | SymbolQuery::Callers { name } => name,
            SymbolQuery::ByFile { .. } => {
                return Err(CtxError::Unimplemented("ByFile".into()));
            }
        };
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<Symbol>> {
            let read = db.begin_read().map_err(store_err)?;
            let table = match read.open_table(SYMBOLS_BY_NAME) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(vec![]),
                Err(e) => return Err(store_err(e)),
            };
            let bytes = table
                .get((key.as_str(), name.as_str()))
                .map_err(store_err)?;
            Ok(match bytes {
                Some(b) => serde_json::from_slice(b.value()).map_err(store_err)?,
                None => vec![],
            })
        })
        .await
        .map_err(|e| CtxError::Store(format!("symbols join: {e}")))?
    }

    async fn record_file_hash(&self, scope: &Scope, file: &str, hash: ContentHash) -> Result<()> {
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        let file = file.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let write = db.begin_write().map_err(store_err)?;
            {
                let mut table = write.open_table(FILE_HASH).map_err(store_err)?;
                table
                    .insert((key.as_str(), file.as_str()), &hash.0)
                    .map_err(store_err)?;
            }
            write.commit().map_err(store_err)
        })
        .await
        .map_err(|e| CtxError::Store(format!("record_file_hash join: {e}")))??;
        Ok(())
    }

    async fn file_hash(&self, scope: &Scope, file: &str) -> Result<Option<ContentHash>> {
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        let file = file.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<ContentHash>> {
            let read = db.begin_read().map_err(store_err)?;
            let table = match read.open_table(FILE_HASH) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
                Err(e) => return Err(store_err(e)),
            };
            Ok(table
                .get((key.as_str(), file.as_str()))
                .map_err(store_err)?
                .map(|v| ContentHash(*v.value())))
        })
        .await
        .map_err(|e| CtxError::Store(format!("file_hash join: {e}")))?
    }

    // reason: the body is long because it needs two separate table scans (ACTIVE and
    // SYMBOLS_BY_NAME) inside a single write transaction; splitting into helpers would
    // require passing the open WriteTransaction across function boundaries, which redb
    // doesn't support cleanly. The logic is well-commented and follows a consistent pattern.
    #[allow(clippy::too_many_lines)]
    async fn clear_file_state(&self, scope: &Scope, file: &str) -> Result<()> {
        let key = Self::scope_key(scope)?;
        let db = self.db.clone();
        let file = file.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let write = db.begin_write().map_err(store_err)?;
            {
                // 1. Remove ACTIVE rows whose ChunkRef.file == file.
                //    Collect hashes to remove first (can't modify table while iterating).
                let mut active = write.open_table(ACTIVE).map_err(store_err)?;
                let mut to_remove: Vec<[u8; 32]> = Vec::new();
                {
                    let low = [0x00u8; 32];
                    let high = [0xffu8; 32];
                    let range_start = (key.as_str(), &low);
                    let range_end = (key.as_str(), &high);
                    let iter = active
                        .range(range_start..=range_end)
                        .map_err(store_err)?;
                    for row in iter {
                        let (k, v) = row.map_err(store_err)?;
                        let (_, hash) = k.value();
                        let bytes = v.value();
                        let chunk_ref: ChunkRef =
                            serde_json::from_slice(bytes).map_err(store_err)?;
                        if chunk_ref.file == file {
                            to_remove.push(*hash);
                        }
                    }
                }
                for hash in to_remove {
                    active
                        .remove((key.as_str(), &hash))
                        .map_err(store_err)?;
                }

                // 2. Clear symbols: scan SYMBOLS_BY_NAME and filter out entries in `file`.
                //    Rewrite rows that still have symbols for other files; delete empty ones.
                let mut symbols_by_name =
                    write.open_table(SYMBOLS_BY_NAME).map_err(store_err)?;
                let mut updates: Vec<(String, Vec<Symbol>)> = Vec::new();
                let mut deletions: Vec<String> = Vec::new();
                {
                    // Range covers all entries with this scope prefix.
                    // "\u{10ffff}" is the highest valid Unicode scalar — chosen so that
                    // (key, "\u{10ffff}") is lexicographically >= any real symbol name.
                    let low_key = (key.as_str(), "");
                    let high_key = (key.as_str(), "\u{10ffff}");
                    let iter = symbols_by_name
                        .range(low_key..=high_key)
                        .map_err(store_err)?;
                    for row in iter {
                        let (k, v) = row.map_err(store_err)?;
                        let (_, name) = k.value();
                        let stored: Vec<Symbol> =
                            serde_json::from_slice(v.value()).map_err(store_err)?;
                        let filtered: Vec<Symbol> = stored
                            .into_iter()
                            .filter(|s| s.file != file)
                            .collect();
                        if filtered.is_empty() {
                            deletions.push(name.to_string());
                        } else {
                            updates.push((name.to_string(), filtered));
                        }
                    }
                }
                for name in deletions {
                    symbols_by_name
                        .remove((key.as_str(), name.as_str()))
                        .map_err(store_err)?;
                }
                for (name, filtered) in updates {
                    let bytes = serde_json::to_vec(&filtered).map_err(store_err)?;
                    symbols_by_name
                        .insert((key.as_str(), name.as_str()), bytes.as_slice())
                        .map_err(store_err)?;
                }
            }
            write.commit().map_err(store_err)?;
            Ok(())
        })
        .await
        .map_err(|e| CtxError::Store(format!("clear_file_state join: {e}")))??;
        Ok(())
    }
}

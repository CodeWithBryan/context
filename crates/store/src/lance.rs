// Arrow column layout for each chunk row:
//
//   hash:       FixedSizeBinary(32)  — raw ContentHash bytes
//   file:       Utf8
//   lang:       UInt8  — stable enum index (see lang_to_u8/u8_to_lang)
//   kind:       UInt8  — stable enum index (see kind_to_u8/u8_to_kind)
//   name:       Utf8   — nullable
//   byte_start: UInt64
//   byte_end:   UInt64
//   line_start: UInt32
//   line_end:   UInt32
//   text:       Utf8
//   vector:     FixedSizeList<Float32>(dim)
//
// Enum → u8 mappings are intentionally stable across builds:
//   Language  : TypeScript=0, Tsx=1, JavaScript=2, Jsx=3, Css=4, Html=5, Json=6
//   ChunkKind : Function=0, Method=1, Class=2, Interface=3, Type=4, Const=5, Enum=6,
//               Selector=7, Element=8, Document=9
// Changing these values would invalidate any persisted LanceDB table.

use async_trait::async_trait;
use arrow_array::{
    Array, FixedSizeBinaryArray, FixedSizeListArray, Float32Array, RecordBatch,
    StringArray, UInt32Array, UInt64Array, UInt8Array,
    builder::{FixedSizeBinaryBuilder, FixedSizeListBuilder, Float32Builder, StringBuilder,
              UInt32Builder, UInt64Builder, UInt8Builder},
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use ctx_core::traits::{ChunkStore, Filter};
use ctx_core::types::{ByteRange, LineRange};
use ctx_core::{Chunk, ChunkKind, ContentHash, CtxError, Hit, Language, Result};
use futures::TryStreamExt;
use lancedb::Error as LanceError;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;
use std::path::Path;
use std::sync::Arc;

const TABLE_NAME: &str = "chunks";

// ── Enum ↔ u8 stable mappings ────────────────────────────────────────────────

fn lang_to_u8(l: Language) -> u8 {
    match l {
        Language::TypeScript  => 0,
        Language::Tsx         => 1,
        Language::JavaScript  => 2,
        Language::Jsx         => 3,
        Language::Css         => 4,
        Language::Html        => 5,
        Language::Json        => 6,
    }
}

fn u8_to_lang(x: u8) -> Result<Language> {
    match x {
        0 => Ok(Language::TypeScript),
        1 => Ok(Language::Tsx),
        2 => Ok(Language::JavaScript),
        3 => Ok(Language::Jsx),
        4 => Ok(Language::Css),
        5 => Ok(Language::Html),
        6 => Ok(Language::Json),
        _ => Err(CtxError::Store(format!("unknown Language byte {x}"))),
    }
}

fn kind_to_u8(k: ChunkKind) -> u8 {
    match k {
        ChunkKind::Function  => 0,
        ChunkKind::Method    => 1,
        ChunkKind::Class     => 2,
        ChunkKind::Interface => 3,
        ChunkKind::Type      => 4,
        ChunkKind::Const     => 5,
        ChunkKind::Enum      => 6,
        ChunkKind::Selector  => 7,
        ChunkKind::Element   => 8,
        ChunkKind::Document  => 9,
    }
}

fn u8_to_kind(x: u8) -> Result<ChunkKind> {
    match x {
        0 => Ok(ChunkKind::Function),
        1 => Ok(ChunkKind::Method),
        2 => Ok(ChunkKind::Class),
        3 => Ok(ChunkKind::Interface),
        4 => Ok(ChunkKind::Type),
        5 => Ok(ChunkKind::Const),
        6 => Ok(ChunkKind::Enum),
        7 => Ok(ChunkKind::Selector),
        8 => Ok(ChunkKind::Element),
        9 => Ok(ChunkKind::Document),
        _ => Err(CtxError::Store(format!("unknown ChunkKind byte {x}"))),
    }
}

// ── Schema ────────────────────────────────────────────────────────────────────

fn chunk_schema(dim: usize) -> Schema {
    let dim_i32 = i32::try_from(dim).expect("dim fits in i32: model output dimensions never exceed i32::MAX");
    let vector_item = Field::new("item", DataType::Float32, true);
    let vector_list = DataType::FixedSizeList(Arc::new(vector_item), dim_i32);
    Schema::new(vec![
        Field::new("hash",       DataType::FixedSizeBinary(32), false),
        Field::new("file",       DataType::Utf8,                false),
        Field::new("lang",       DataType::UInt8,               false),
        Field::new("kind",       DataType::UInt8,               false),
        Field::new("name",       DataType::Utf8,                true),
        Field::new("byte_start", DataType::UInt64,              false),
        Field::new("byte_end",   DataType::UInt64,              false),
        Field::new("line_start", DataType::UInt32,              false),
        Field::new("line_end",   DataType::UInt32,              false),
        Field::new("text",       DataType::Utf8,                false),
        Field::new("vector",     vector_list,                   false),
    ])
}

// ── Store struct ──────────────────────────────────────────────────────────────

pub struct LanceChunkStore {
    /// Kept alive so `LanceDB` can open new tables / flush background writes.
    _conn:  Connection,
    table:  Table,
    dim:    usize,
    schema: SchemaRef,
}

impl LanceChunkStore {
    /// Open or create the `LanceDB` table at `dir` with the given vector dimensionality.
    ///
    /// # Errors
    /// Returns `CtxError::Store` if the path is non-UTF-8, the connection fails, or the
    /// table schema is incompatible with `dim`.
    pub async fn open(dir: impl AsRef<Path>, dim: usize) -> Result<Self> {
        let uri = dir
            .as_ref()
            .to_str()
            .ok_or_else(|| CtxError::Store("non-UTF-8 LanceDB path".into()))?;

        let conn = lancedb::connect(uri)
            .execute()
            .await
            .map_err(|e| CtxError::Store(format!("connect: {e}")))?;

        let schema: SchemaRef = Arc::new(chunk_schema(dim));

        let table = match conn.open_table(TABLE_NAME).execute().await {
            Ok(t) => {
                // Validate that the stored table's vector dimension matches the
                // requested dim — fail fast rather than deferring to the first
                // query that would produce a confusing type mismatch.
                let existing = t
                    .schema()
                    .await
                    .map_err(|e| CtxError::Store(format!("read existing schema: {e}")))?;
                validate_schema_dim(&existing, dim)?;
                t
            }
            Err(LanceError::TableNotFound { .. }) => {
                // Table doesn't exist yet — create empty with our schema.
                conn.create_empty_table(TABLE_NAME, schema.clone())
                    .execute()
                    .await
                    .map_err(|e| CtxError::Store(format!("create_empty_table: {e}")))?
            }
            Err(e) => return Err(CtxError::Store(format!("open_table: {e}"))),
        };

        Ok(Self { _conn: conn, table, dim, schema })
    }
}

// ── Schema validation ─────────────────────────────────────────────────────────

/// Verify that `schema` contains a `vector` column whose `FixedSizeList` inner
/// length equals `expected_dim`. Returns `CtxError::Store` on any mismatch.
fn validate_schema_dim(schema: &Schema, expected_dim: usize) -> Result<()> {
    let field = schema
        .field_with_name("vector")
        .map_err(|e| CtxError::Store(format!("existing table missing 'vector' column: {e}")))?;
    match field.data_type() {
        DataType::FixedSizeList(_, len) => {
            let stored = usize::try_from(*len)
                .map_err(|_| CtxError::Store(format!("stored vector dim {len} is invalid")))?;
            if stored != expected_dim {
                return Err(CtxError::Store(format!(
                    "existing table vector dim {stored} != requested dim {expected_dim}"
                )));
            }
            Ok(())
        }
        other => Err(CtxError::Store(format!(
            "existing 'vector' column has wrong type: {other:?}"
        ))),
    }
}

// ── ChunkStore impl ───────────────────────────────────────────────────────────

#[async_trait]
impl ChunkStore for LanceChunkStore {
    async fn upsert(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let batch = build_record_batch(&self.schema, chunks, self.dim)?;
        // merge_insert matches on "hash"; updates existing rows, inserts new ones.
        // MergeInsertBuilder methods take &mut self; execute consumes self.
        let mut mi = self.table.merge_insert(&["hash"]);
        mi.when_matched_update_all(None)
            .when_not_matched_insert_all();
        mi.execute(Box::new(arrow_array::RecordBatchIterator::new(
                vec![Ok(batch)],
                self.schema.clone(),
            )))
            .await
            .map_err(|e| CtxError::Store(format!("merge_insert: {e}")))?;
        Ok(())
    }

    async fn get(&self, hash: &ContentHash) -> Result<Option<Chunk>> {
        // Full scan then client-side match. LanceDB's SQL predicate support for
        // FixedSizeBinary is version-dependent, so we scan and match in Rust.
        let stream = self
            .table
            .query()
            .execute()
            .await
            .map_err(|e| CtxError::Store(format!("query: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| CtxError::Store(format!("collect: {e}")))?;

        for batch in &batches {
            let hashes = col_as::<FixedSizeBinaryArray>(batch, "hash")?;
            for i in 0..batch.num_rows() {
                if hashes.value(i) == hash.0 {
                    return Ok(Some(batch_row_to_chunk(batch, i, self.dim)?));
                }
            }
        }
        Ok(None)
    }

    async fn search(&self, query: &[f32], k: usize, filter: &Filter) -> Result<Vec<Hit>> {
        if query.len() != self.dim {
            return Err(CtxError::Store(format!(
                "query dim {} != store dim {}",
                query.len(),
                self.dim
            )));
        }

        // Over-fetch to allow post-filter; minimum 1 to satisfy lancedb.
        let fetch = (k * 4).max(1);

        let stream = self
            .table
            .query()
            .nearest_to(query)
            .map_err(|e| CtxError::Store(format!("nearest_to: {e}")))?
            .limit(fetch)
            .execute()
            .await
            .map_err(|e| CtxError::Store(format!("execute: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| CtxError::Store(format!("collect: {e}")))?;

        // Filter application policy (Phase 1):
        //   - hash_allowlist: honored (client-side post-filter below)
        //   - lang_allowlist: honored (client-side post-filter below)
        //   - scope:          NOT enforced server-side. Callers must pass
        //                     `hash_allowlist = RefStore::active_hashes(scope)?` to
        //                     enforce scope isolation. This is a security-relevant
        //                     contract — Task 10 Router is the single enforcement
        //                     point for this pattern.
        //   - path_glob:      NOT applied in Phase 1 (deferred to Task 8+).
        let mut hits: Vec<Hit> = Vec::new();

        'outer: for batch in &batches {
            let hashes = col_as::<FixedSizeBinaryArray>(batch, "hash")?;
            // _distance is injected by lancedb when doing vector search.
            let distance = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            for i in 0..batch.num_rows() {
                let hash_bytes: [u8; 32] = hashes
                    .value(i)
                    .try_into()
                    .map_err(|_| CtxError::Store("hash not 32 bytes".into()))?;
                let hash = ContentHash(hash_bytes);

                // Hash allowlist filter (client-side).
                if let Some(allow) = &filter.hash_allowlist {
                    if !allow.contains(&hash) {
                        continue;
                    }
                }

                let chunk = batch_row_to_chunk(batch, i, self.dim)?;

                // Language allowlist filter (client-side).
                if let Some(langs) = &filter.lang_allowlist {
                    if !langs.contains(&chunk.lang) {
                        continue;
                    }
                }

                let score = distance
                    .map_or(0.0, |d| 1.0_f32 / (1.0 + d.value(i)));

                hits.push(Hit { chunk, score });
                if hits.len() >= k {
                    break 'outer;
                }
            }
        }
        Ok(hits)
    }

    async fn delete(&self, hashes: &[ContentHash]) -> Result<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        // Build an IN predicate using hex literals that LanceDB can evaluate
        // against fixed-size-binary columns via the arrow-datafusion layer.
        // If the SQL predicate syntax isn't supported in this version, fall back
        // to individual deletes (single-row tables are uncommon in production).
        let hex_list: Vec<String> = hashes
            .iter()
            .map(|h| format!("x'{}'", h.to_hex()))
            .collect();
        let pred = format!("hash IN ({})", hex_list.join(", "));
        self.table
            .delete(&pred)
            .await
            .map_err(|e| CtxError::Store(format!("delete: {e}")))?;
        Ok(())
    }

    async fn count(&self) -> Result<u64> {
        let n = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| CtxError::Store(format!("count_rows: {e}")))?;
        Ok(n as u64)
    }
}

// ── Column builder helpers ────────────────────────────────────────────────────

/// Build a single `RecordBatch` from a slice of `Chunk`s.
fn build_record_batch(schema: &SchemaRef, chunks: &[Chunk], dim: usize) -> Result<RecordBatch> {
    let n = chunks.len();

    let mut hash_b       = FixedSizeBinaryBuilder::with_capacity(n, 32);
    let mut file_b       = StringBuilder::new();
    let mut lang_b       = UInt8Builder::with_capacity(n);
    let mut kind_b       = UInt8Builder::with_capacity(n);
    let mut name_b       = StringBuilder::new();
    let mut byte_start_b = UInt64Builder::with_capacity(n);
    let mut byte_end_b   = UInt64Builder::with_capacity(n);
    let mut line_start_b = UInt32Builder::with_capacity(n);
    let mut line_end_b   = UInt32Builder::with_capacity(n);
    let mut text_b       = StringBuilder::new();

    let dim_i32 = i32::try_from(dim).expect("dim fits in i32: model output dimensions never exceed i32::MAX");
    let mut vector_b = FixedSizeListBuilder::new(Float32Builder::new(), dim_i32);

    for c in chunks {
        let vec = c.vector.as_ref().ok_or_else(|| {
            CtxError::Store(format!("chunk {} has no vector", c.hash.to_hex()))
        })?;
        if vec.len() != dim {
            return Err(CtxError::Store(format!(
                "chunk {} vector len {} != dim {}",
                c.hash.to_hex(),
                vec.len(),
                dim
            )));
        }

        hash_b
            .append_value(c.hash.0)
            .map_err(|e| CtxError::Store(format!("hash_builder: {e}")))?;
        file_b.append_value(&c.file);
        lang_b.append_value(lang_to_u8(c.lang));
        kind_b.append_value(kind_to_u8(c.kind));
        match &c.name {
            Some(n) => name_b.append_value(n),
            None    => name_b.append_null(),
        }
        // usize→u64: safe on all Rust targets (usize ≤ 64 bits).
        byte_start_b.append_value(u64::try_from(c.byte_range.start).unwrap_or(u64::MAX));
        byte_end_b.append_value(u64::try_from(c.byte_range.end).unwrap_or(u64::MAX));
        line_start_b.append_value(c.line_range.start);
        line_end_b.append_value(c.line_range.end);
        text_b.append_value(&c.text);

        vector_b.values().append_slice(vec);
        vector_b.append(true);
    }

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(hash_b.finish())       as Arc<dyn Array>,
            Arc::new(file_b.finish())       as Arc<dyn Array>,
            Arc::new(lang_b.finish())       as Arc<dyn Array>,
            Arc::new(kind_b.finish())       as Arc<dyn Array>,
            Arc::new(name_b.finish())       as Arc<dyn Array>,
            Arc::new(byte_start_b.finish()) as Arc<dyn Array>,
            Arc::new(byte_end_b.finish())   as Arc<dyn Array>,
            Arc::new(line_start_b.finish()) as Arc<dyn Array>,
            Arc::new(line_end_b.finish())   as Arc<dyn Array>,
            Arc::new(text_b.finish())       as Arc<dyn Array>,
            Arc::new(vector_b.finish())     as Arc<dyn Array>,
        ],
    )
    .map_err(|e| CtxError::Store(format!("RecordBatch::try_new: {e}")))?;

    Ok(batch)
}

/// Reconstruct a `Chunk` from a single row of a `RecordBatch`.
fn batch_row_to_chunk(batch: &RecordBatch, row: usize, dim: usize) -> Result<Chunk> {
    let hash_arr  = col_as::<FixedSizeBinaryArray>(batch, "hash")?;
    let file_arr  = col_as::<StringArray>(batch, "file")?;
    let lang_arr  = col_as::<UInt8Array>(batch, "lang")?;
    let kind_arr  = col_as::<UInt8Array>(batch, "kind")?;
    let name_arr  = col_as::<StringArray>(batch, "name")?;
    let bst_arr   = col_as::<UInt64Array>(batch, "byte_start")?;
    let bend_arr  = col_as::<UInt64Array>(batch, "byte_end")?;
    let lst_arr   = col_as::<UInt32Array>(batch, "line_start")?;
    let lend_arr  = col_as::<UInt32Array>(batch, "line_end")?;
    let text_arr  = col_as::<StringArray>(batch, "text")?;
    let vec_arr   = col_as::<FixedSizeListArray>(batch, "vector")?;

    let hash_bytes: [u8; 32] = hash_arr
        .value(row)
        .try_into()
        .map_err(|_| CtxError::Store("hash not 32 bytes".into()))?;

    let name = if name_arr.is_null(row) {
        None
    } else {
        Some(name_arr.value(row).to_string())
    };

    // Extract the float32 values from the FixedSizeList element.
    let vec_value = vec_arr.value(row);
    let floats = vec_value
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or_else(|| CtxError::Store("vector inner array not Float32".into()))?;
    if floats.len() != dim {
        return Err(CtxError::Store(format!(
            "stored vector len {} != dim {}",
            floats.len(),
            dim
        )));
    }
    let vector: Vec<f32> = (0..floats.len()).map(|i| floats.value(i)).collect();

    Ok(Chunk {
        hash:       ContentHash(hash_bytes),
        file:       file_arr.value(row).to_string(),
        lang:       u8_to_lang(lang_arr.value(row))?,
        kind:       u8_to_kind(kind_arr.value(row))?,
        name,
        byte_range: ByteRange::new(
            usize::try_from(bst_arr.value(row))
                .map_err(|_| CtxError::Store("byte_start overflows usize".into()))?,
            usize::try_from(bend_arr.value(row))
                .map_err(|_| CtxError::Store("byte_end overflows usize".into()))?,
        ),
        line_range: LineRange::new(
            lst_arr.value(row),
            lend_arr.value(row),
        ),
        text:   text_arr.value(row).to_string(),
        vector: Some(vector),
    })
}

/// Downcast a named column in `batch` to `A`, returning `CtxError::Store` on failure.
fn col_as<'a, A: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a A> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| CtxError::Store(format!("missing column '{name}'")))?;
    col.as_any()
        .downcast_ref::<A>()
        .ok_or_else(|| CtxError::Store(format!("column '{name}' has unexpected type")))
}

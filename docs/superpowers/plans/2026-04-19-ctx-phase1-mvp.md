# ctx — Phase 1 MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a single-binary, local-only, pure-Rust MCP server (`ctx`) that indexes TypeScript/TSX/JS/JSX/CSS/HTML/JSON codebases, answers semantic + structural queries over stdio MCP, and runs comfortably on an M4 MacBook Air with 24 GB RAM. The first real-world target is `~/Development/hotwash/hotwash` (~51k LOC across apps/packages/services in a Bun monorepo).

**Architecture:** Cargo workspace with a trait-driven core. Everything is in-process — no external services. Tree-sitter parses source into function/class chunks. `fastembed-rs` (ONNX + CoreML) produces embeddings locally. LanceDB (embedded) stores vectors; `redb` stores the symbol/reference graph and per-worktree active chunk set. `notify` watches the filesystem and incrementally re-indexes via blake3 content hashing so branch switches and worktrees don't trigger re-embeds. A spawned `tsserver` child process supplies real LSP-grade TypeScript symbols. `rmcp` exposes six tools over stdio: `semantic_search`, `find_definition`, `find_references`, `find_callers`, `get_chunk`, `repo_status`. All storage backends sit behind `ChunkStore` / `RefStore` / `Embedder` / `Reranker` / `Auth` traits so Phase 4 can swap in Qdrant + Postgres + HTTP transport without touching the query, index, or MCP crates.

**Tech Stack:** Rust 1.94 · tokio · `rmcp` · `tree-sitter` (+ ts/tsx/js/jsx/css/html/json grammars) · `fastembed-rs` (nomic-embed-code INT8 ONNX) · `lancedb` embedded · `redb` · `tantivy` · `notify` · `gitoxide` · `blake3` · `anyhow` · `thiserror` · `clap` · `serde` · `tracing` · `tracing-subscriber`

---

## Scope Boundaries

**In scope (Phase 1):**
- Single Rust binary `ctx` with `init`, `index`, `serve`, `status` subcommands
- Indexing TS/TSX/JS/JSX/CSS/HTML/JSON under a single repo root
- Semantic search via local embeddings + ANN
- TypeScript call/def/ref graph via spawned `tsserver`
- CSS/HTML tree-sitter-query-based symbol lookup (class/id/tag)
- File watcher with content-hash dedup
- Per-repo data at `~/.ctx/repos/<blake3-of-abs-path>/`
- MCP stdio transport only

**Out of scope (deferred to Phase 2+):**
- Reranker crate (hooks present, impl later)
- Commit/lineage indexing
- Multi-branch or all-branches queries
- Remote/HTTP transport
- Multi-tenant / auth / capability tokens
- Qdrant/Postgres store backends
- Languages beyond the listed seven
- `diff_context`, `path_between_symbols` tools

---

## File Structure

```
context/
├── Cargo.toml                       # workspace
├── Cargo.lock
├── rust-toolchain.toml              # pin 1.94 stable
├── .gitignore                       # add target/, .ctx/, *.onnx, etc.
├── README.md                        # placeholder
├── crates/
│   ├── core/                        # traits + types, no I/O
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types.rs             # Chunk, ChunkRef, Symbol, Vector, Hit
│   │       ├── scope.rs             # Scope { tenant, repo, worktree, branch }
│   │       ├── hash.rs              # blake3 helpers for content + paths
│   │       ├── error.rs             # CtxError, Result alias
│   │       └── traits.rs            # ChunkStore, RefStore, Embedder, Auth
│   ├── parse/                       # tree-sitter chunking
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── languages.rs         # Language enum + detection by extension
│   │       ├── ts.rs                # typescript/tsx queries
│   │       ├── js.rs                # javascript/jsx queries
│   │       ├── css.rs               # css class/id/selector queries
│   │       ├── html.rs              # html tag/class/id queries
│   │       ├── json.rs              # json whole-file chunk
│   │       └── chunker.rs           # Chunker::chunk_file(path, bytes) -> Vec<Chunk>
│   ├── embed/                       # embedder impl
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── fastembed.rs         # FastembedEmbedder (nomic-embed-code)
│   ├── store/                       # lance + redb impls behind traits
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── lance.rs             # LanceChunkStore
│   │       └── redb_refs.rs         # RedbRefStore (refs + symbols + active set)
│   ├── symbol/                      # tsserver bridge + tree-sitter symbols
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── tsserver.rs          # spawn + stdin/stdout protocol
│   │       ├── tree_symbols.rs      # CSS/HTML symbols via tree-sitter
│   │       └── extractor.rs         # unified Symbol extraction
│   ├── index/                       # pipeline orchestration
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── pipeline.rs          # full + incremental index passes
│   │       └── merkle.rs            # per-file hash map persisted to redb
│   ├── watch/                       # notify wrapper
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── debounce.rs          # coalesce rapid file events
│   ├── query/                       # retrieval router
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── router.rs            # semantic_search, find_callers, ...
│   ├── mcp/                         # rmcp server
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs            # CtxMcpServer struct + service impl
│   │       └── tools.rs             # tool handlers mapping to query crate
│   └── cli/                         # `ctx` binary entrypoint
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── commands/
│           │   ├── mod.rs
│           │   ├── init.rs
│           │   ├── index.rs
│           │   ├── serve.rs
│           │   └── status.rs
│           └── config.rs            # ~/.ctx/config.toml + per-repo resolution
├── xtask/                           # helper binary for dev chores
│   ├── Cargo.toml
│   └── src/main.rs                  # download-models, lint, fmt-check
└── docs/
    └── superpowers/
        └── plans/
            └── 2026-04-19-ctx-phase1-mvp.md   # this document
```

Each crate has one clear responsibility. Trait definitions (`core`) and impls (`store`, `embed`) are separated so additional impls drop in later without touching consumers.

---

## Task 0: Workspace Bootstrap

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `README.md`

- [ ] **Step 1: Write workspace manifest**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
    "crates/core",
    "crates/parse",
    "crates/embed",
    "crates/store",
    "crates/symbol",
    "crates/index",
    "crates/watch",
    "crates/query",
    "crates/mcp",
    "crates/cli",
    "xtask",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.94"
authors = ["Bryan Gillespie"]
repository = "https://github.com/CodeWithBryan/context"
license-file = "LICENSE"

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
blake3 = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
async-trait = "0.1"
clap = { version = "4", features = ["derive"] }
toml = "0.8"
dirs = "5"
rmcp = { version = "0.2", features = ["server", "transport-io"] }
tree-sitter = "0.23"
tree-sitter-typescript = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-css = "0.23"
tree-sitter-html = "0.23"
tree-sitter-json = "0.23"
notify = "6"
notify-debouncer-full = "0.3"
lancedb = "0.11"
arrow-array = "52"
arrow-schema = "52"
redb = "2"
tantivy = "0.22"
fastembed = "4"
gix = "0.66"
once_cell = "1"
parking_lot = "0.12"
walkdir = "2"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
```

Verify versions published on crates.io before final commit — adjust if a major bump has landed.

- [ ] **Step 2: Pin the toolchain**

Create `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.94"
components = ["rustfmt", "clippy"]
profile = "default"
```

- [ ] **Step 3: Write .gitignore**

```
target/
Cargo.lock.bak
.ctx/
*.onnx
*.gguf
.DS_Store
.idea/
.vscode/
```

Note: `Cargo.lock` is committed for binary crates.

- [ ] **Step 3b: Write placeholder LICENSE**

`Cargo.toml` sets `license-file.workspace = true`, so a file must exist before `cargo check` runs.

Create `LICENSE`:

```
UNDECIDED — all rights reserved.

This project is currently private. A final license will be selected before any
public release. Until then, no rights are granted to copy, modify, distribute,
or use this code.
```

- [ ] **Step 4: Create placeholder crate dirs + README**

For each crate listed in workspace.members, create the directory and a stub `Cargo.toml` + `src/lib.rs` (or `src/main.rs` for `cli` and `xtask`).

Stub crate `Cargo.toml` template:

```toml
[package]
name = "ctx-<crate>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license-file.workspace = true

[dependencies]
# filled in per crate in later tasks
```

Stub `src/lib.rs`:

```rust
//! ctx-<crate> — see plan Task N.
```

`README.md`:

```markdown
# ctx

Local-first, pure-Rust MCP context engine for TypeScript/JS/CSS/HTML codebases.

Early development — not yet usable. See `docs/superpowers/plans/` for the current roadmap.
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check --workspace`
Expected: PASS (every crate is an empty stub, so zero errors).

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "chore: scaffold cargo workspace and crate skeletons"
```

---

## Task 1: Core Types

**Files:**
- Create: `crates/core/Cargo.toml`
- Create: `crates/core/src/lib.rs`
- Create: `crates/core/src/types.rs`
- Create: `crates/core/src/error.rs`
- Create: `crates/core/src/hash.rs`
- Test: `crates/core/tests/types.rs`

- [ ] **Step 1: Fill in core Cargo.toml**

```toml
[package]
name = "ctx-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license-file.workspace = true

[dependencies]
anyhow.workspace = true
thiserror.workspace = true
serde.workspace = true
blake3.workspace = true
async-trait.workspace = true
```

- [ ] **Step 2: Write failing test for ContentHash**

`crates/core/tests/types.rs`:

```rust
use ctx_core::hash::ContentHash;

#[test]
fn content_hash_is_deterministic() {
    let a = ContentHash::of(b"hello world");
    let b = ContentHash::of(b"hello world");
    assert_eq!(a, b);
    assert_eq!(a.to_hex().len(), 64);
}

#[test]
fn content_hash_differs_for_different_input() {
    let a = ContentHash::of(b"hello");
    let b = ContentHash::of(b"world");
    assert_ne!(a, b);
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p ctx-core`
Expected: FAIL — `ContentHash` undefined.

- [ ] **Step 4: Implement `hash.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }
}

impl std::fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ContentHash({}…)", &self.to_hex()[..12])
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(CHARS[(b >> 4) as usize] as char);
        out.push(CHARS[(b & 0x0f) as usize] as char);
    }
    out
}
```

- [ ] **Step 5: Run, expect PASS**

Run: `cargo test -p ctx-core`
Expected: PASS.

- [ ] **Step 6: Add remaining types**

`crates/core/src/types.rs`:

```rust
use crate::hash::ContentHash;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub hash: ContentHash,
    pub file: String,
    pub lang: Language,
    pub kind: ChunkKind,
    pub name: Option<String>,
    pub byte_range: Range<usize>,
    pub line_range: Range<u32>,
    pub text: String,
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Css,
    Html,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkKind {
    Function,
    Method,
    Class,
    Interface,
    Type,
    Const,
    Enum,
    Selector,
    Element,
    Document,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: ChunkKind,
    pub file: String,
    pub line: u32,
    pub container: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkRef {
    pub hash: ContentHash,
    pub file: String,
    pub line_range: Range<u32>,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub chunk: Chunk,
    pub score: f32,
}
```

`crates/core/src/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum CtxError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(String),
    #[error("store: {0}")]
    Store(String),
    #[error("embed: {0}")]
    Embed(String),
    #[error("symbol: {0}")]
    Symbol(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T, E = CtxError> = std::result::Result<T, E>;
```

`crates/core/src/lib.rs`:

```rust
pub mod error;
pub mod hash;
pub mod scope;
pub mod traits;
pub mod types;

pub use error::{CtxError, Result};
pub use hash::ContentHash;
pub use scope::Scope;
pub use types::*;
```

- [ ] **Step 7: Add Scope**

`crates/core/src/scope.rs`:

```rust
use crate::ContentHash;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub tenant: String,     // "local" in Phase 1
    pub repo: RepoId,
    pub worktree: WorktreeId,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub ContentHash);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeId(pub ContentHash);

impl Scope {
    pub fn local(repo_abs_path: &str, worktree_abs_path: &str, branch: Option<String>) -> Self {
        Scope {
            tenant: "local".to_string(),
            repo: RepoId(ContentHash::of(repo_abs_path.as_bytes())),
            worktree: WorktreeId(ContentHash::of(worktree_abs_path.as_bytes())),
            branch,
        }
    }
}
```

- [ ] **Step 8: Commit**

```bash
git add crates/core
git commit -m "feat(core): types, scope, content hash, error enum"
```

---

## Task 2: Trait Definitions

**Files:**
- Create: `crates/core/src/traits.rs`
- Test: `crates/core/tests/traits.rs`

- [ ] **Step 1: Write failing compile check for traits**

`crates/core/tests/traits.rs`:

```rust
use ctx_core::traits::{ChunkStore, Embedder, Filter, RefStore};

// Compile-only: prove the traits are object-safe-ish (boxed behind async-trait).
fn _assert_trait_objects(
    _: Box<dyn ChunkStore>,
    _: Box<dyn RefStore>,
    _: Box<dyn Embedder>,
    _: Filter,
) {}
```

- [ ] **Step 2: Run, expect compile failure**

Run: `cargo test -p ctx-core --no-run`
Expected: FAIL — traits undefined.

- [ ] **Step 3: Implement traits**

`crates/core/src/traits.rs`:

```rust
use crate::{Chunk, ChunkRef, ContentHash, Hit, Result, Scope, Symbol};
use async_trait::async_trait;
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub scope: Option<Scope>,
    pub hash_allowlist: Option<HashSet<ContentHash>>,
    pub lang_allowlist: Option<Vec<crate::Language>>,
    pub path_glob: Option<String>,
}

#[async_trait]
pub trait ChunkStore: Send + Sync {
    async fn upsert(&self, chunks: &[Chunk]) -> Result<()>;
    async fn get(&self, hash: &ContentHash) -> Result<Option<Chunk>>;
    async fn search(&self, query: &[f32], k: usize, filter: &Filter) -> Result<Vec<Hit>>;
    async fn delete(&self, hashes: &[ContentHash]) -> Result<()>;
    async fn count(&self) -> Result<u64>;
}

#[derive(Clone, Debug)]
pub enum SymbolQuery {
    Definition { name: String },
    References { name: String },
    Callers { name: String },
    ByFile { file: String },
}

#[async_trait]
pub trait RefStore: Send + Sync {
    async fn bind(&self, scope: &Scope, refs: &[ChunkRef]) -> Result<()>;
    async fn active_hashes(&self, scope: &Scope) -> Result<HashSet<ContentHash>>;
    async fn upsert_symbols(&self, scope: &Scope, symbols: &[Symbol]) -> Result<()>;
    async fn symbols(&self, scope: &Scope, q: SymbolQuery) -> Result<Vec<Symbol>>;
    async fn record_file_hash(&self, scope: &Scope, file: &str, hash: ContentHash) -> Result<()>;
    async fn file_hash(&self, scope: &Scope, file: &str) -> Result<Option<ContentHash>>;
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}
```

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p ctx-core`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core
git commit -m "feat(core): ChunkStore, RefStore, Embedder traits"
```

---

## Task 3: Tree-sitter Chunker

**Files:**
- Modify: `crates/parse/Cargo.toml`
- Create: `crates/parse/src/lib.rs`
- Create: `crates/parse/src/languages.rs`
- Create: `crates/parse/src/chunker.rs`
- Create: `crates/parse/src/ts.rs` / `js.rs` / `css.rs` / `html.rs` / `json.rs`
- Test: `crates/parse/tests/fixtures/*.{ts,tsx,css,html,json}`
- Test: `crates/parse/tests/chunker.rs`

- [ ] **Step 1: Populate Cargo.toml**

```toml
[package]
name = "ctx-parse"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license-file.workspace = true

[dependencies]
ctx-core = { path = "../core" }
tree-sitter.workspace = true
tree-sitter-typescript.workspace = true
tree-sitter-javascript.workspace = true
tree-sitter-css.workspace = true
tree-sitter-html.workspace = true
tree-sitter-json.workspace = true
anyhow.workspace = true
once_cell.workspace = true
```

- [ ] **Step 2: Write failing test for TS function chunking**

Create fixture `crates/parse/tests/fixtures/sample.ts`:

```typescript
export function greet(name: string): string {
    return `hello, ${name}`;
}

export class Counter {
    private n = 0;
    increment(): number {
        this.n += 1;
        return this.n;
    }
}
```

Create `crates/parse/tests/chunker.rs`:

```rust
use ctx_parse::Chunker;
use std::fs;

#[test]
fn chunks_ts_functions_and_methods() {
    let src = fs::read_to_string("tests/fixtures/sample.ts").unwrap();
    let chunks = Chunker::new().chunk("tests/fixtures/sample.ts", src.as_bytes()).unwrap();

    let names: Vec<_> = chunks.iter().filter_map(|c| c.name.clone()).collect();
    assert!(names.contains(&"greet".to_string()));
    assert!(names.contains(&"Counter".to_string()));
    assert!(names.contains(&"increment".to_string()));
}
```

- [ ] **Step 3: Run, expect failure**

Run: `cargo test -p ctx-parse`
Expected: FAIL — `Chunker` undefined.

- [ ] **Step 4: Implement `languages.rs`**

```rust
use ctx_core::Language;
use std::path::Path;

pub fn detect(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "ts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "js" | "mjs" | "cjs" => Language::JavaScript,
        "jsx" => Language::Jsx,
        "css" => Language::Css,
        "html" | "htm" => Language::Html,
        "json" => Language::Json,
        _ => return None,
    })
}

pub fn tree_sitter_language(lang: Language) -> tree_sitter::Language {
    match lang {
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::JavaScript | Language::Jsx => tree_sitter_javascript::LANGUAGE.into(),
        Language::Css => tree_sitter_css::LANGUAGE.into(),
        Language::Html => tree_sitter_html::LANGUAGE.into(),
        Language::Json => tree_sitter_json::LANGUAGE.into(),
    }
}
```

- [ ] **Step 5: Implement `chunker.rs`**

```rust
use crate::{languages, ts, css, html, json};
use ctx_core::{Chunk, ChunkKind, ContentHash, Language, Result, CtxError};
use std::ops::Range;
use std::path::Path;
use tree_sitter::{Parser, Tree};

pub struct Chunker {}

impl Chunker {
    pub fn new() -> Self { Self {} }

    pub fn chunk(&self, file: &str, bytes: &[u8]) -> Result<Vec<Chunk>> {
        let path = Path::new(file);
        let lang = languages::detect(path)
            .ok_or_else(|| CtxError::Parse(format!("no grammar for {file}")))?;
        let mut parser = Parser::new();
        parser.set_language(&languages::tree_sitter_language(lang))
            .map_err(|e| CtxError::Parse(e.to_string()))?;
        let tree = parser.parse(bytes, None)
            .ok_or_else(|| CtxError::Parse("parse returned None".into()))?;

        let regions = match lang {
            Language::TypeScript | Language::Tsx
            | Language::JavaScript | Language::Jsx => ts::extract(&tree, bytes),
            Language::Css => css::extract(&tree, bytes),
            Language::Html => html::extract(&tree, bytes),
            Language::Json => json::extract(bytes),
        };

        Ok(regions.into_iter().map(|r| Chunk {
            hash: ContentHash::of(&bytes[r.byte_range.clone()]),
            file: file.to_string(),
            lang,
            kind: r.kind,
            name: r.name,
            byte_range: r.byte_range.clone(),
            line_range: r.line_range,
            text: std::str::from_utf8(&bytes[r.byte_range]).unwrap_or("").to_string(),
            vector: None,
        }).collect())
    }
}

pub struct Region {
    pub kind: ChunkKind,
    pub name: Option<String>,
    pub byte_range: Range<usize>,
    pub line_range: Range<u32>,
}
```

- [ ] **Step 6: Implement `ts.rs` with tree-sitter queries**

```rust
use crate::chunker::Region;
use ctx_core::ChunkKind;
use once_cell::sync::Lazy;
use tree_sitter::{Query, QueryCursor, Tree};

const TS_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @function
(method_definition name: (property_identifier) @name) @method
(class_declaration name: (type_identifier) @name) @class
(interface_declaration name: (type_identifier) @name) @interface
(type_alias_declaration name: (type_identifier) @name) @type
(enum_declaration name: (identifier) @name) @enum
(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @arrow
"#;

static QUERY: Lazy<Query> = Lazy::new(|| {
    Query::new(&tree_sitter_typescript::LANGUAGE_TSX.into(), TS_QUERY).expect("bad ts query")
});

pub fn extract(tree: &Tree, src: &[u8]) -> Vec<Region> {
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    for m in cursor.matches(&QUERY, tree.root_node(), src) {
        let mut kind: Option<ChunkKind> = None;
        let mut name: Option<String> = None;
        let mut node: Option<tree_sitter::Node> = None;
        for cap in m.captures {
            let cap_name = QUERY.capture_names()[cap.index as usize];
            match cap_name {
                "name" => name = Some(cap.node.utf8_text(src).unwrap_or("").to_string()),
                "function" | "arrow" => { kind = Some(ChunkKind::Function); node = Some(cap.node); }
                "method" => { kind = Some(ChunkKind::Method); node = Some(cap.node); }
                "class" => { kind = Some(ChunkKind::Class); node = Some(cap.node); }
                "interface" => { kind = Some(ChunkKind::Interface); node = Some(cap.node); }
                "type" => { kind = Some(ChunkKind::Type); node = Some(cap.node); }
                "enum" => { kind = Some(ChunkKind::Enum); node = Some(cap.node); }
                _ => {}
            }
        }
        if let (Some(kind), Some(node)) = (kind, node) {
            out.push(Region {
                kind,
                name,
                byte_range: node.byte_range(),
                line_range: node.start_position().row as u32..node.end_position().row as u32 + 1,
            });
        }
    }
    out
}
```

(CSS/HTML/JSON follow the same shape with their own queries — see Task 3.9 below.)

- [ ] **Step 7: Implement `css.rs`, `html.rs`, `json.rs`, and `lib.rs`**

`css.rs` — capture `(rule_set (selectors) @sel)` nodes, kind `Selector`, name = selectors text trimmed.

`html.rs` — capture `(element (start_tag (tag_name) @tag))`, kind `Element`, name = tag name.

`json.rs` — return one `Document` region spanning the whole file.

`crates/parse/src/lib.rs`:

```rust
mod languages;
mod chunker;
pub mod ts;
pub mod js;
pub mod css;
pub mod html;
pub mod json;

pub use chunker::Chunker;
pub use languages::detect;
```

- [ ] **Step 8: Run tests, iterate until PASS**

Run: `cargo test -p ctx-parse`
Expected: PASS. Add additional fixtures for TSX React components, CSS, HTML once the TS test passes — each a separate test, same red-green-refactor rhythm.

- [ ] **Step 9: Commit**

```bash
git add crates/parse
git commit -m "feat(parse): tree-sitter chunker for ts/tsx/js/css/html/json"
```

---

## Task 4: Fastembed Embedder

**Files:**
- Modify: `crates/embed/Cargo.toml`
- Create: `crates/embed/src/lib.rs`
- Create: `crates/embed/src/fastembed.rs`
- Test: `crates/embed/tests/fastembed_smoke.rs`

- [ ] **Step 1: Populate Cargo.toml**

```toml
[package]
name = "ctx-embed"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license-file.workspace = true

[dependencies]
ctx-core = { path = "../core" }
fastembed.workspace = true
tokio.workspace = true
async-trait.workspace = true
anyhow.workspace = true
```

- [ ] **Step 2: Write failing smoke test**

`crates/embed/tests/fastembed_smoke.rs`:

```rust
use ctx_core::traits::Embedder;
use ctx_embed::FastembedEmbedder;

#[tokio::test]
#[ignore = "downloads ~150MB model — run with `cargo test --ignored -p ctx-embed`"]
async fn embeds_two_strings_with_expected_dim() {
    let embedder = FastembedEmbedder::new_default().await.unwrap();
    let out = embedder.embed(&[
        "function add(a: number, b: number) { return a + b; }".to_string(),
        "const sum = (x, y) => x + y".to_string(),
    ]).await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].len(), embedder.dim());
    assert!(out[0].len() >= 384);
}
```

- [ ] **Step 3: Implement `FastembedEmbedder`**

`crates/embed/src/fastembed.rs`:

```rust
use async_trait::async_trait;
use ctx_core::{Embedder, Result, CtxError};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use parking_lot::Mutex;
use std::sync::Arc;

pub struct FastembedEmbedder {
    inner: Arc<Mutex<TextEmbedding>>,
    dim: usize,
    model_id: String,
}

impl FastembedEmbedder {
    pub async fn new_default() -> Result<Self> {
        // NomicEmbedTextV15 is the current best small code-capable default in fastembed
        // Swap to a code-specialized ONNX via UserDefinedEmbeddingModel once nomic-embed-code
        // ONNX is packaged upstream.
        let model = EmbeddingModel::NomicEmbedTextV15;
        let te = tokio::task::spawn_blocking(move || {
            TextEmbedding::try_new(InitOptions::new(model).with_show_download_progress(true))
        }).await
            .map_err(|e| CtxError::Embed(e.to_string()))?
            .map_err(|e| CtxError::Embed(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(te)),
            dim: 768,
            model_id: "nomic-embed-text-v1.5".to_string(),
        })
    }
}

#[async_trait]
impl Embedder for FastembedEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let te = self.inner.clone();
        let owned: Vec<String> = texts.to_vec();
        tokio::task::spawn_blocking(move || {
            let guard = te.lock();
            guard.embed(owned, None)
        }).await
            .map_err(|e| CtxError::Embed(e.to_string()))?
            .map_err(|e| CtxError::Embed(e.to_string()))
    }

    fn dim(&self) -> usize { self.dim }
    fn model_id(&self) -> &str { &self.model_id }
}
```

`crates/embed/src/lib.rs`:

```rust
mod fastembed;
pub use fastembed::FastembedEmbedder;
```

- [ ] **Step 4: Run ignored test manually, expect PASS**

Run: `cargo test --ignored -p ctx-embed -- --nocapture`
Expected: PASS on first run (downloads model), cache hit on subsequent runs.

- [ ] **Step 5: Commit**

```bash
git add crates/embed
git commit -m "feat(embed): local fastembed embedder (nomic-embed-text-v1.5)"
```

**Follow-up (Phase 2):** package nomic-embed-code ONNX as a fastembed `UserDefinedEmbeddingModel` — better code recall, same dimensions.

---

## Task 5: redb Ref Store

**Files:**
- Modify: `crates/store/Cargo.toml`
- Create: `crates/store/src/lib.rs`
- Create: `crates/store/src/redb_refs.rs`
- Test: `crates/store/tests/redb_refs.rs`

- [ ] **Step 1: Populate store Cargo.toml**

```toml
[package]
name = "ctx-store"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license-file.workspace = true

[dependencies]
ctx-core = { path = "../core" }
redb.workspace = true
lancedb.workspace = true
arrow-array.workspace = true
arrow-schema.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
async-trait.workspace = true
anyhow.workspace = true
tracing.workspace = true
```

- [ ] **Step 2: Write failing tests for ref store**

`crates/store/tests/redb_refs.rs`:

```rust
use ctx_core::{ChunkRef, ContentHash, Scope, Symbol, ChunkKind, SymbolQuery, traits::RefStore};
use ctx_store::RedbRefStore;
use tempfile::tempdir;

fn scope() -> Scope { Scope::local("/repo", "/repo", Some("main".into())) }

#[tokio::test]
async fn bind_and_list_active_hashes() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let s = scope();
    let hash = ContentHash::of(b"x");
    store.bind(&s, &[ChunkRef { hash, file: "a.ts".into(), line_range: 0..10 }]).await.unwrap();
    let active = store.active_hashes(&s).await.unwrap();
    assert!(active.contains(&hash));
}

#[tokio::test]
async fn upsert_and_find_symbol_definition() {
    let dir = tempdir().unwrap();
    let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
    let s = scope();
    store.upsert_symbols(&s, &[Symbol {
        name: "greet".into(), kind: ChunkKind::Function,
        file: "a.ts".into(), line: 3, container: None,
    }]).await.unwrap();
    let out = store.symbols(&s, SymbolQuery::Definition { name: "greet".into() }).await.unwrap();
    assert_eq!(out.len(), 1);
}
```

(Add `tempfile = "3"` to `[dev-dependencies]`.)

- [ ] **Step 3: Run, expect failure**

Run: `cargo test -p ctx-store`
Expected: FAIL — `RedbRefStore` undefined.

- [ ] **Step 4: Implement `RedbRefStore`**

`crates/store/src/redb_refs.rs`:

```rust
use async_trait::async_trait;
use ctx_core::{ChunkRef, ContentHash, CtxError, Result, Scope, Symbol,
               traits::{RefStore, SymbolQuery}};
use redb::{Database, TableDefinition};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

// Tables keyed by scope_hex then inner key.
const REFS: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("refs");
const ACTIVE: TableDefinition<(&str, &[u8; 32]), &[u8]> = TableDefinition::new("active");
const SYMBOLS_BY_NAME: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("symbols_by_name");
const FILE_HASH: TableDefinition<(&str, &str), &[u8; 32]> = TableDefinition::new("file_hash");

pub struct RedbRefStore {
    db: Arc<Database>,
}

impl RedbRefStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path).map_err(|e| CtxError::Store(e.to_string()))?;
        Ok(Self { db: Arc::new(db) })
    }

    fn scope_key(scope: &Scope) -> String {
        format!("{}:{}:{}:{}",
            scope.tenant,
            scope.repo.0.to_hex(),
            scope.worktree.0.to_hex(),
            scope.branch.as_deref().unwrap_or("_none"))
    }
}

#[async_trait]
impl RefStore for RedbRefStore {
    async fn bind(&self, scope: &Scope, refs: &[ChunkRef]) -> Result<()> {
        let key = Self::scope_key(scope);
        let db = self.db.clone();
        let refs = refs.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let write = db.begin_write().map_err(|e| CtxError::Store(e.to_string()))?;
            {
                let mut active = write.open_table(ACTIVE).map_err(|e| CtxError::Store(e.to_string()))?;
                for r in refs {
                    let bytes = serde_json::to_vec(&r).map_err(|e| CtxError::Store(e.to_string()))?;
                    active.insert((key.as_str(), &r.hash.0), bytes.as_slice())
                        .map_err(|e| CtxError::Store(e.to_string()))?;
                }
            }
            write.commit().map_err(|e| CtxError::Store(e.to_string()))?;
            Ok(())
        }).await.map_err(|e| CtxError::Store(e.to_string()))??;
        Ok(())
    }

    async fn active_hashes(&self, scope: &Scope) -> Result<HashSet<ContentHash>> {
        let key = Self::scope_key(scope);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<HashSet<ContentHash>> {
            let read = db.begin_read().map_err(|e| CtxError::Store(e.to_string()))?;
            let table = read.open_table(ACTIVE).map_err(|e| CtxError::Store(e.to_string()))?;
            let mut out = HashSet::new();
            for row in table.range((key.as_str(), &[0u8; 32])..=(key.as_str(), &[0xffu8; 32])) {
                let ((_, h), _) = row.map_err(|e| CtxError::Store(e.to_string()))?;
                out.insert(ContentHash(*h));
            }
            Ok(out)
        }).await.map_err(|e| CtxError::Store(e.to_string()))?
    }

    async fn upsert_symbols(&self, scope: &Scope, symbols: &[Symbol]) -> Result<()> {
        // One row per (scope, name), value is a JSON array of Symbols with that name.
        // Read-modify-write; fine for MVP scale.
        let key = Self::scope_key(scope);
        let db = self.db.clone();
        let syms = symbols.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let write = db.begin_write().map_err(|e| CtxError::Store(e.to_string()))?;
            {
                let mut table = write.open_table(SYMBOLS_BY_NAME)
                    .map_err(|e| CtxError::Store(e.to_string()))?;
                use std::collections::HashMap;
                let mut grouped: HashMap<String, Vec<Symbol>> = HashMap::new();
                for s in syms { grouped.entry(s.name.clone()).or_default().push(s); }
                for (name, mut new_syms) in grouped {
                    let existing = table.get((key.as_str(), name.as_str()))
                        .map_err(|e| CtxError::Store(e.to_string()))?;
                    if let Some(bytes) = existing {
                        let mut prior: Vec<Symbol> = serde_json::from_slice(bytes.value())
                            .map_err(|e| CtxError::Store(e.to_string()))?;
                        prior.append(&mut new_syms);
                        new_syms = prior;
                    }
                    let bytes = serde_json::to_vec(&new_syms)
                        .map_err(|e| CtxError::Store(e.to_string()))?;
                    table.insert((key.as_str(), name.as_str()), bytes.as_slice())
                        .map_err(|e| CtxError::Store(e.to_string()))?;
                }
            }
            write.commit().map_err(|e| CtxError::Store(e.to_string()))?;
            Ok(())
        }).await.map_err(|e| CtxError::Store(e.to_string()))??;
        Ok(())
    }

    async fn symbols(&self, scope: &Scope, q: SymbolQuery) -> Result<Vec<Symbol>> {
        let key = Self::scope_key(scope);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<Symbol>> {
            let read = db.begin_read().map_err(|e| CtxError::Store(e.to_string()))?;
            let table = read.open_table(SYMBOLS_BY_NAME).map_err(|e| CtxError::Store(e.to_string()))?;
            let (name, _want_refs) = match q {
                SymbolQuery::Definition { name } => (name, false),
                SymbolQuery::References { name } | SymbolQuery::Callers { name } => (name, true),
                SymbolQuery::ByFile { .. } => { return Ok(vec![]); } // Phase 2
            };
            let bytes = table.get((key.as_str(), name.as_str()))
                .map_err(|e| CtxError::Store(e.to_string()))?;
            Ok(match bytes {
                Some(b) => serde_json::from_slice(b.value())
                    .map_err(|e| CtxError::Store(e.to_string()))?,
                None => vec![],
            })
        }).await.map_err(|e| CtxError::Store(e.to_string()))?
    }

    async fn record_file_hash(&self, scope: &Scope, file: &str, hash: ContentHash) -> Result<()> {
        let key = Self::scope_key(scope);
        let db = self.db.clone();
        let file = file.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let write = db.begin_write().map_err(|e| CtxError::Store(e.to_string()))?;
            {
                let mut table = write.open_table(FILE_HASH).map_err(|e| CtxError::Store(e.to_string()))?;
                table.insert((key.as_str(), file.as_str()), &hash.0)
                    .map_err(|e| CtxError::Store(e.to_string()))?;
            }
            write.commit().map_err(|e| CtxError::Store(e.to_string()))?;
            Ok(())
        }).await.map_err(|e| CtxError::Store(e.to_string()))??;
        Ok(())
    }

    async fn file_hash(&self, scope: &Scope, file: &str) -> Result<Option<ContentHash>> {
        let key = Self::scope_key(scope);
        let db = self.db.clone();
        let file = file.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<ContentHash>> {
            let read = db.begin_read().map_err(|e| CtxError::Store(e.to_string()))?;
            let table = read.open_table(FILE_HASH).map_err(|e| CtxError::Store(e.to_string()))?;
            Ok(table.get((key.as_str(), file.as_str()))
                .map_err(|e| CtxError::Store(e.to_string()))?
                .map(|v| ContentHash(*v.value())))
        }).await.map_err(|e| CtxError::Store(e.to_string()))?
    }
}
```

(Adjust exact redb API as the 2.x type signatures evolve — same logic.)

- [ ] **Step 5: Run, expect PASS**

Run: `cargo test -p ctx-store redb_refs`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/store
git commit -m "feat(store): redb-backed RefStore with scope-keyed tables"
```

---

## Task 6: LanceDB Chunk Store

**Files:**
- Create: `crates/store/src/lance.rs`
- Modify: `crates/store/src/lib.rs`
- Test: `crates/store/tests/lance_store.rs`

- [ ] **Step 1: Write failing test**

`crates/store/tests/lance_store.rs`:

```rust
use ctx_core::{Chunk, ChunkKind, ContentHash, Language, traits::{ChunkStore, Filter}};
use ctx_store::LanceChunkStore;
use tempfile::tempdir;

fn chunk(i: u8, text: &str, vec: Vec<f32>) -> Chunk {
    Chunk {
        hash: ContentHash([i; 32]),
        file: format!("f{i}.ts"),
        lang: Language::TypeScript,
        kind: ChunkKind::Function,
        name: Some(format!("fn{i}")),
        byte_range: 0..text.len(),
        line_range: 0..1,
        text: text.into(),
        vector: Some(vec),
    }
}

#[tokio::test]
async fn upsert_and_search_returns_nearest() {
    let dir = tempdir().unwrap();
    let store = LanceChunkStore::open(dir.path(), 4).await.unwrap();
    store.upsert(&[
        chunk(1, "greet", vec![1.0, 0.0, 0.0, 0.0]),
        chunk(2, "farewell", vec![0.0, 1.0, 0.0, 0.0]),
    ]).await.unwrap();
    let hits = store.search(&[0.99, 0.01, 0.0, 0.0], 1, &Filter::default()).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk.name.as_deref(), Some("fn1"));
}
```

- [ ] **Step 2: Implement `LanceChunkStore`**

See `crates/store/src/lance.rs` with Arrow schema `(hash: FixedSizeBinary(32), file: Utf8, lang: Utf8, kind: Utf8, name: Utf8, text: Utf8, vector: FixedSizeList<Float32>(dim))`. Create table on `open` if missing; use `IVF_PQ` index with `num_partitions = 256` once row count > 10k.

Key methods:
- `upsert` — convert `Chunk`s to a RecordBatch, call `table.merge_insert` keyed on `hash`.
- `search` — `table.vector_search(q).limit(k).execute()`, then filter by `Filter.hash_allowlist` in memory post-query.
- `delete` — `table.delete(predicate)`.
- `count` — `table.count_rows()`.

(Full code omitted here — follow LanceDB's Rust example repo and the Arrow `FixedSizeListBuilder` pattern.)

- [ ] **Step 3: Run, expect PASS**

Run: `cargo test -p ctx-store lance_store`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/store
git commit -m "feat(store): embedded LanceDB ChunkStore with vector search"
```

---

## Task 7: Symbol Extraction (tsserver + tree-sitter)

**Files:**
- Modify: `crates/symbol/Cargo.toml`
- Create: `crates/symbol/src/lib.rs`
- Create: `crates/symbol/src/tsserver.rs`
- Create: `crates/symbol/src/tree_symbols.rs`
- Create: `crates/symbol/src/extractor.rs`
- Test: `crates/symbol/tests/tsserver.rs` (requires Node)
- Test: `crates/symbol/tests/tree_symbols.rs`

- [ ] **Step 1: Write failing test for CSS class extraction**

```rust
use ctx_symbol::tree_symbols::extract_css;

#[test]
fn extracts_css_class_selectors() {
    let css = ".btn-primary { color: red; } #hero h1 { font-size: 2rem; }";
    let symbols = extract_css("theme.css", css.as_bytes()).unwrap();
    let names: Vec<_> = symbols.iter().map(|s| s.name.clone()).collect();
    assert!(names.iter().any(|n| n.contains(".btn-primary")));
}
```

- [ ] **Step 2: Implement tree-sitter symbol extractors**

Use the existing chunker regions plus tree-sitter queries that target `(class_selector)`, `(id_selector)`, `(tag_name)`. Map each to a `Symbol` with `kind = Selector | Element`.

- [ ] **Step 3: Implement tsserver bridge**

`tsserver.rs` spawns `node <path>/tsserver` as a child, communicates via the TSServer protocol (newline-delimited JSON with `Content-Length` headers).

Key operations for MVP:
- `open` a file
- `navtree` (returns the whole symbol outline per file → drives `Symbol` list)
- `references` (for `find_references` and `find_callers`)
- `definition`

Locate `tsserver` via `node_modules/.bin/tsserver` under the repo root first, falling back to a bundled path set by `CTX_TSSERVER_PATH` env var. If neither exists, downgrade gracefully — log a warning and return empty results rather than fail the server.

- [ ] **Step 4: Write integration test against hotwash (ignored)**

```rust
#[tokio::test]
#[ignore = "requires tsserver + hotwash repo"]
async fn finds_greet_symbol_in_hotwash() { /* … */ }
```

- [ ] **Step 5: Run tests (tree-symbols PASS, tsserver ignored)**

Run: `cargo test -p ctx-symbol`
Expected: PASS for tree-symbols tests.

- [ ] **Step 6: Commit**

```bash
git add crates/symbol
git commit -m "feat(symbol): tsserver bridge + tree-sitter selector/element symbols"
```

---

## Task 8: Merkle State + Indexing Pipeline

**Files:**
- Modify: `crates/index/Cargo.toml`
- Create: `crates/index/src/lib.rs`
- Create: `crates/index/src/merkle.rs`
- Create: `crates/index/src/pipeline.rs`
- Test: `crates/index/tests/pipeline.rs`

- [ ] **Step 1: Populate Cargo.toml**

```toml
[dependencies]
ctx-core = { path = "../core" }
ctx-parse = { path = "../parse" }
ctx-embed = { path = "../embed" }
ctx-store = { path = "../store" }
ctx-symbol = { path = "../symbol" }
tokio.workspace = true
anyhow.workspace = true
walkdir.workspace = true
ignore = "0.4"        # .gitignore-aware traversal — critical for skipping node_modules
tracing.workspace = true
async-trait.workspace = true
```

Use the `ignore::WalkBuilder` crate instead of raw `walkdir` for the traversal step; it respects `.gitignore`, `.ignore`, and global `$HOME/.gitignore_global` out of the box. Keep `walkdir` only if needed for glob-free subtasks.

- [ ] **Step 2: Write failing pipeline integration test**

Fixture: a tiny 3-file repo in `tests/fixtures/minirepo/` with `a.ts`, `b.ts`, `c.css`.

Test verifies:
- After `Pipeline::full_index(root)`, `ChunkStore::count()` > 0.
- After modifying `a.ts`, `Pipeline::incremental(root)` touches only `a.ts` (no re-embed of `b.ts` or `c.css`).

- [ ] **Step 3: Implement pipeline**

```rust
pub struct Pipeline<C, R, E> {
    pub chunks: C,
    pub refs: R,
    pub embedder: E,
}

impl<C: ChunkStore, R: RefStore, E: Embedder> Pipeline<C, R, E> {
    pub async fn full_index(&self, scope: &Scope, root: &Path) -> Result<IndexReport> { ... }
    pub async fn incremental(&self, scope: &Scope, root: &Path, changed: &[PathBuf]) -> Result<IndexReport> { ... }
    async fn index_file(&self, scope: &Scope, path: &Path) -> Result<FileReport> {
        // 1. read bytes
        // 2. content hash the file
        // 3. compare to RefStore::file_hash — if same, skip
        // 4. chunk via Chunker
        // 5. compare each chunk hash to ChunkStore::get — skip unchanged chunks for embedding
        // 6. embed only new chunks
        // 7. upsert chunks
        // 8. bind refs into RefStore (replaces prior refs for this file)
        // 9. extract + upsert symbols
        // 10. record file hash
    }
}
```

Use `walkdir` with a `.gitignore`-aware filter (`gix-ignore` or a hand-rolled `.gitignore` parser) so `node_modules`, `dist`, `target`, etc. are skipped.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p ctx-index`
Expected: PASS (with the embedder either real or a deterministic mock embedder you plug in for tests — see note below).

**Test-only Embedder mock**: define `MockEmbedder` in a `test-support` module that returns deterministic vectors based on text hash. Lets the pipeline test stay fast and offline.

- [ ] **Step 5: Commit**

```bash
git add crates/index
git commit -m "feat(index): file-level + chunk-level hash dedup, full + incremental passes"
```

---

## Task 9: File Watcher

**Files:**
- Modify: `crates/watch/Cargo.toml`
- Create: `crates/watch/src/lib.rs`
- Create: `crates/watch/src/debounce.rs`
- Test: `crates/watch/tests/watch.rs`

- [ ] **Step 1: Populate Cargo.toml**

```toml
[dependencies]
ctx-core = { path = "../core" }
notify.workspace = true
notify-debouncer-full.workspace = true
tokio.workspace = true
tracing.workspace = true
```

- [ ] **Step 2: Write failing test**

Write a test that creates a temp dir, spawns a watcher with a 200 ms debounce, touches two files rapidly, and asserts the watcher yields a single deduplicated batch containing both paths.

- [ ] **Step 3: Implement `Watcher::start(root) -> tokio::sync::mpsc::Receiver<BatchedEvent>`**

Use `notify-debouncer-full` with `Duration::from_millis(200)` and forward batched events over an `mpsc` channel. Filter out events inside `node_modules`, `.git`, `target`, `dist`.

- [ ] **Step 4: Run, expect PASS**
- [ ] **Step 5: Commit**

```bash
git add crates/watch
git commit -m "feat(watch): notify + debounce batched filesystem watcher"
```

---

## Task 10: Query Router

**Files:**
- Modify: `crates/query/Cargo.toml`
- Create: `crates/query/src/lib.rs`
- Create: `crates/query/src/router.rs`
- Test: `crates/query/tests/router.rs`

- [ ] **Step 1: Define the `Router` API**

```rust
pub struct Router<C, R, E> {
    chunks: Arc<C>,
    refs:   Arc<R>,
    embed:  Arc<E>,
}

impl<C, R, E> Router<C, R, E>
where C: ChunkStore, R: RefStore, E: Embedder {
    pub async fn semantic_search(&self, scope: &Scope, q: &str, k: usize) -> Result<Vec<Hit>> {
        let v = self.embed.embed(&[q.to_string()]).await?.remove(0);
        let mut filter = Filter::default();
        filter.scope = Some(scope.clone());
        filter.hash_allowlist = Some(self.refs.active_hashes(scope).await?);
        self.chunks.search(&v, k, &filter).await
    }
    pub async fn find_definition(&self, scope: &Scope, name: &str) -> Result<Vec<Symbol>> { ... }
    pub async fn find_references(&self, scope: &Scope, name: &str) -> Result<Vec<Symbol>> { ... }
    pub async fn find_callers(&self, scope: &Scope, name: &str) -> Result<Vec<Symbol>> { ... }
    pub async fn get_chunk(&self, hash: ContentHash) -> Result<Option<Chunk>> { ... }
    pub async fn status(&self, scope: &Scope) -> Result<Status> { ... }
}
```

- [ ] **Step 2: Write tests with mock stores backing the router**

Instantiate the router over in-memory mock stores to test the routing logic in isolation.

- [ ] **Step 3: Implement, run, commit**

```bash
git add crates/query
git commit -m "feat(query): Router for semantic + symbol queries, active-hash filtering"
```

---

## Task 11: MCP Server

**Files:**
- Modify: `crates/mcp/Cargo.toml`
- Create: `crates/mcp/src/lib.rs`
- Create: `crates/mcp/src/server.rs`
- Create: `crates/mcp/src/tools.rs`
- Test: `crates/mcp/tests/mcp_smoke.rs`

- [ ] **Step 1: Define tool schemas**

Six tools, each a struct with `#[derive(Deserialize, schemars::JsonSchema)]` for params:

- `semantic_search { query: String, k: u32 }`
- `find_definition { name: String }`
- `find_references { name: String }`
- `find_callers { name: String }`
- `get_chunk { hash: String }`     // hex-encoded ContentHash
- `repo_status {}`

- [ ] **Step 2: Implement service per `rmcp` 0.2 pattern**

```rust
use rmcp::{ServiceExt, RoleServer, service::Service, transport::stdio};

pub struct CtxMcpServer { pub router: Arc<Router<...>>, pub scope: Scope }

#[rmcp::tool_router]
impl CtxMcpServer {
    #[rmcp::tool(description = "Semantic code search")]
    async fn semantic_search(&self, params: SemanticSearchArgs) -> Result<Value, ToolError> { ... }
    // ... etc
}
```

Wire `CtxMcpServer` into `rmcp`'s service + stdio transport on `CtxMcpServer::serve_stdio`.

- [ ] **Step 3: Write smoke test that spawns the server over stdio and calls `semantic_search`**

Use `rmcp`'s client side to round-trip a request in the test. Fail if response is not well-formed JSON or the tool isn't listed.

- [ ] **Step 4: Commit**

```bash
git add crates/mcp
git commit -m "feat(mcp): rmcp stdio server exposing 6 query tools"
```

---

## Task 12: CLI + Config

**Files:**
- Modify: `crates/cli/Cargo.toml`
- Create: `crates/cli/src/main.rs`
- Create: `crates/cli/src/config.rs`
- Create: `crates/cli/src/commands/{mod.rs,init.rs,index.rs,serve.rs,status.rs}`
- Test: `crates/cli/tests/cli_smoke.rs`

- [ ] **Step 1: Populate Cargo.toml**

```toml
[[bin]]
name = "ctx"
path = "src/main.rs"

[dependencies]
ctx-core = { path = "../core" }
ctx-embed = { path = "../embed" }
ctx-store = { path = "../store" }
ctx-index = { path = "../index" }
ctx-watch = { path = "../watch" }
ctx-symbol = { path = "../symbol" }
ctx-query = { path = "../query" }
ctx-mcp = { path = "../mcp" }
clap.workspace = true
tokio.workspace = true
serde.workspace = true
toml.workspace = true
dirs.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
gix.workspace = true
```

- [ ] **Step 2: Implement subcommands**

```rust
#[derive(clap::Parser)]
#[command(name = "ctx", version, about = "Local context engine for TS/JS/CSS/HTML")]
struct Cli { #[command(subcommand)] cmd: Cmd }

#[derive(clap::Subcommand)]
enum Cmd {
    /// Initialize per-repo state under ~/.ctx/repos/<hash>/
    Init { #[arg(default_value = ".")] path: PathBuf },
    /// Full (re)index; exits when done.
    Index { #[arg(default_value = ".")] path: PathBuf, #[arg(long)] full: bool },
    /// Start watcher + MCP stdio server.
    Serve { #[arg(default_value = ".")] path: PathBuf },
    /// Print index health.
    Status { #[arg(default_value = ".")] path: PathBuf },
}
```

`serve` flow:
1. Resolve repo abs path → `Scope::local`.
2. Open `LanceChunkStore`, `RedbRefStore`, `FastembedEmbedder`.
3. Spawn `Watcher` task; route events to `Pipeline::incremental`.
4. Build `Router`, then `CtxMcpServer::serve_stdio`.

`index --full` does the same setup, then `Pipeline::full_index` and exits.

- [ ] **Step 3: Smoke test the CLI with `assert_cmd`**

- [ ] **Step 4: Commit**

```bash
git add crates/cli
git commit -m "feat(cli): ctx init|index|serve|status"
```

---

## Task 13: End-to-End Integration Test

**Files:**
- Create: `crates/cli/tests/hotwash_e2e.rs`

- [ ] **Step 1: Write ignored e2e test**

```rust
#[tokio::test]
#[ignore = "requires ~/Development/hotwash/hotwash checked out"]
async fn indexes_hotwash_and_answers_semantic_query() {
    let repo = dirs::home_dir().unwrap().join("Development/hotwash/hotwash");
    // 1. ctx init repo
    // 2. ctx index --full repo
    // 3. Spawn ctx serve as a child, talk MCP over stdio
    // 4. Call semantic_search("dashboard layout component") → assert ≥1 hit whose file is under apps/ or packages/
    // 5. Call find_definition("AppRouter") → assert ≥1 hit
    // 6. Touch one file, wait 1s, call semantic_search again with near-identical query → assert still works
}
```

- [ ] **Step 2: Run with `cargo test --ignored -p ctx-cli hotwash_e2e`**

Expected: PASS end-to-end. Capture wall-clock for full index and for `semantic_search` latency; record in plan review notes for the Phase 1 retro.

- [ ] **Step 3: Commit**

```bash
git add crates/cli/tests/hotwash_e2e.rs
git commit -m "test(e2e): hotwash full-index + MCP roundtrip"
```

---

## Exit Criteria for Phase 1

A Phase 1 build is "done" when all of the below are true:

1. `cargo install --path crates/cli` produces a `ctx` binary.
2. `ctx index --full ~/Development/hotwash/hotwash` completes with `count > 0` chunks and < 5 GB RAM at peak.
3. `ctx serve ~/Development/hotwash/hotwash` registers over MCP stdio with Claude Code; `tools/list` shows all six tools.
4. `semantic_search` returns relevant chunks for three hand-written TS queries within p50 < 300 ms on the M4.
5. `find_definition` and `find_callers` return correct file+line for a known TS symbol in hotwash.
6. Editing a TS file triggers incremental re-index within 2 seconds, observable via `ctx status`.
7. The full test suite (`cargo test --workspace`) passes.
8. `cargo clippy --workspace --all-targets -- -D warnings` passes.
9. `cargo fmt --all -- --check` passes.

---

## Risk Register

- **LanceDB + Arrow version drift** — pin explicitly in `Cargo.toml`; re-check before every phase gate.
- **fastembed model naming** — NomicEmbedTextV15 is fine for MVP; upgrade to nomic-embed-code once packaged upstream.
- **tsserver protocol idiosyncrasies** — keep the bridge isolated behind the `Symbol` trait so we can swap it for a pure-Rust TS resolver (`oxc`) later.
- **redb concurrent-writer contention** under burst indexing — batch writes at the pipeline level to keep one writer open per pass.
- **`.gitignore` handling** — underestimating this produces 10× index blow-up from `node_modules`. Use `gix-ignore` if hand-rolled parsing gets fiddly.
- **License undecided** — placeholder `LICENSE` file containing `UNDECIDED — all rights reserved, private project` until the decision is made.

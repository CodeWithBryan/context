# ctx

Local-first, pure-Rust MCP context engine for TypeScript/JS/CSS/HTML codebases.

`ctx` watches your source tree, extracts symbols via tree-sitter, embeds them
with a local model, and serves results through the Model Context Protocol so
editors and AI agents can query your codebase without sending code to the cloud.
**Phase 1: local-only, TS/JS/CSS/HTML support only. Not yet suitable for production use.**

## Quick start

```bash
cargo install --path crates/cli
ctx init .
ctx serve .
```

## Status

Phase 1 MVP is complete and verified end-to-end. See the implementation plan for
scope and known limitations:
[`docs/superpowers/plans/2026-04-19-ctx-phase1-mvp.md`](docs/superpowers/plans/2026-04-19-ctx-phase1-mvp.md)

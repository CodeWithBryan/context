# ctx

Local-first, pure-Rust MCP context engine for TypeScript/JS/CSS/HTML codebases.

`ctx` watches your source tree, extracts symbols via tree-sitter, embeds them
with a local model, and serves results through the Model Context Protocol so
editors and AI agents can query your codebase without sending code to the cloud.

**Phase 1: local-only, TS/JS/CSS/HTML support only. Not yet suitable for production use.**

## Install (macOS, Apple Silicon)

```sh
curl -L https://github.com/CodeWithBryan/context/releases/latest/download/ctx-latest-aarch64-apple-darwin.tar.gz | tar xz -C /usr/local/bin ctx
```

Binaries are Developer ID signed + notarized — no Gatekeeper workaround needed.
Intel Macs and Linux are out of scope for Phase 1.

## First run

```sh
cd my-project
ctx init .
ctx index .      # first time: ~150 MB model download + ~7 min indexing
```

## Wire it into your agent

### Claude Code

```sh
claude mcp add ctx -- ctx serve .
```

### Codex CLI

```sh
codex mcp add ctx -- ctx serve .
```

Both commands register `ctx` as a stdio MCP server scoped to the current repo.
Run them from the project root after `ctx init .` + `ctx index .`.

## Update

```sh
ctx update --force
```

## Benchmark

A/B bench: runs fixed question set through `claude --print` twice — once with
no MCP, once with `ctx` — and compares wall time, tokens, cost.

**Requires:** `bun`, `claude`, `ctx` in PATH. Target repo already indexed
(`ctx init . && ctx index .`).

```sh
# defaults: 5 built-in questions, 1 run, concurrency 4
bun run scripts/bench-ctx.ts ~/path/to/repo

# custom questions file (one per line, `#` comments ignored), 3 runs averaged,
# serial (concurrency 1 = most stable numbers)
bun run scripts/bench-ctx.ts ~/path/to/repo \
  --questions scripts/bench-questions.txt \
  --runs 3 \
  --concurrency 1
```

Prints a markdown table per question + totals (wall clock, input/output
tokens, USD). Kill any running `ctx serve` before starting — the bench spawns
its own shared HTTP server so the embedder loads once.

## Status

Phase 1 MVP is complete and verified end-to-end. See the implementation plan for
scope and known limitations:
[`docs/superpowers/plans/2026-04-19-ctx-phase1-mvp.md`](docs/superpowers/plans/2026-04-19-ctx-phase1-mvp.md)

Full install options and MCP config JSON: [`INSTALL.md`](INSTALL.md)

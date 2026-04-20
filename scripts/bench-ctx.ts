#!/usr/bin/env bun
/**
 * A/B benchmark: does `ctx` actually help Claude Code?
 *
 * Runs a fixed set of questions through `claude --print` twice — once with
 * no MCP servers, once with the `ctx` MCP server — and compares wall clock
 * time, input/output tokens, and total cost.
 *
 * Usage:
 *   bun run scripts/bench-ctx.ts <repo-dir> [--questions path.txt] [--concurrency N]
 *
 * Example:
 *   bun run scripts/bench-ctx.ts ~/Development/hotwash/hotwash --concurrency 4
 *
 * Requires:
 *   - `claude` CLI (Claude Code) in PATH
 *   - `ctx` CLI in PATH (for the with-ctx leg)
 *   - The repo has already been indexed (`ctx init . && ctx index .`)
 */

import { spawn } from "bun";
import { existsSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// ──────────────────────────────────────────────────────────────────────────
// Config
// ──────────────────────────────────────────────────────────────────────────

const DEFAULT_QUESTIONS = [
  "Where is session authentication implemented in this repo? Give me a file path and line number.",
  "Find the function that sets the forwarded auth header on gateway requests.",
  "List three places where better-auth session tokens are parsed from Set-Cookie headers.",
  "Summarize how the admin dashboard fetches organization metrics, citing the function name and file.",
  "What does the MetricCard component do? Reference file + line number.",
];

const PER_QUESTION_TIMEOUT_MS = 180_000;

// ──────────────────────────────────────────────────────────────────────────
// Parse args
// ──────────────────────────────────────────────────────────────────────────

const args = process.argv.slice(2);
if (args.length === 0 || args[0] === "--help" || args[0] === "-h") {
  console.error(
    "usage: bun run scripts/bench-ctx.ts <repo-dir> [--questions path.txt]",
  );
  process.exit(2);
}

const repoDir = args[0]!;
let questions = DEFAULT_QUESTIONS;
let concurrency = 4;

const qFileIdx = args.indexOf("--questions");
if (qFileIdx >= 0 && args[qFileIdx + 1]) {
  const path = args[qFileIdx + 1]!;
  if (!existsSync(path)) {
    console.error(`questions file not found: ${path}`);
    process.exit(2);
  }
  questions = (await Bun.file(path).text())
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0 && !l.startsWith("#"));
}

const cIdx = args.indexOf("--concurrency");
if (cIdx >= 0 && args[cIdx + 1]) {
  const n = Number.parseInt(args[cIdx + 1]!, 10);
  if (!Number.isFinite(n) || n < 1) {
    console.error(`invalid --concurrency value: ${args[cIdx + 1]}`);
    process.exit(2);
  }
  concurrency = n;
}

if (!existsSync(repoDir)) {
  console.error(`repo dir not found: ${repoDir}`);
  process.exit(2);
}

// ──────────────────────────────────────────────────────────────────────────
// Tooling sanity checks
// ──────────────────────────────────────────────────────────────────────────

async function checkBin(bin: string): Promise<void> {
  const proc = Bun.spawn([bin, "--version"], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const code = await proc.exited;
  if (code !== 0) {
    console.error(`\`${bin}\` not found or failed to run`);
    process.exit(2);
  }
}

await checkBin("claude");
await checkBin("ctx");

// ──────────────────────────────────────────────────────────────────────────
// Write temp MCP configs
// ──────────────────────────────────────────────────────────────────────────

const tmp = mkdtempSync(join(tmpdir(), "ctx-bench-"));
const baselineCfg = join(tmp, "baseline.json");
const withCtxCfg = join(tmp, "with-ctx.json");
writeFileSync(baselineCfg, JSON.stringify({ mcpServers: {} }));
writeFileSync(
  withCtxCfg,
  JSON.stringify({
    mcpServers: {
      ctx: {
        command: "ctx",
        args: ["serve"],
        type: "stdio",
      },
    },
  }),
);

// ──────────────────────────────────────────────────────────────────────────
// Run one question
// ──────────────────────────────────────────────────────────────────────────

interface RunResult {
  ok: boolean;
  wallMs: number;
  inputTokens: number;
  cacheReadTokens: number;
  outputTokens: number;
  costUsd: number;
  numTurns: number;
  answer: string;
  error?: string;
}

async function runOne(
  mcpCfg: string,
  question: string,
): Promise<RunResult> {
  const start = performance.now();
  // stream-json emits one JSON object per line: system init, assistant
  // (with usage), tool_use, tool_result, ..., result. Summing usage across
  // all assistant events gives accurate CUMULATIVE token counts — the plain
  // `json` format reports only the final turn's usage.
  const proc = spawn({
    cmd: [
      "claude",
      "--print",
      "--verbose",
      "--output-format",
      "stream-json",
      "--mcp-config",
      mcpCfg,
      "--permission-mode",
      "bypassPermissions",
      question,
    ],
    cwd: repoDir,
    stdout: "pipe",
    stderr: "pipe",
  });

  const timeout = setTimeout(() => {
    try {
      proc.kill();
    } catch {}
  }, PER_QUESTION_TIMEOUT_MS);

  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  clearTimeout(timeout);

  const wallMs = Math.round(performance.now() - start);

  if (code !== 0) {
    return {
      ok: false,
      wallMs,
      inputTokens: 0,
      cacheReadTokens: 0,
      outputTokens: 0,
      costUsd: 0,
      numTurns: 0,
      answer: "",
      error: `exit ${code}: ${stderr.slice(-400)}`,
    };
  }

  // Accumulate across all events
  let inputTokens = 0;
  let cacheReadTokens = 0;
  let cacheCreationTokens = 0;
  let outputTokens = 0;
  let costUsd = 0;
  let numTurns = 0;
  let answer = "";
  let isError = false;

  for (const line of stdout.split("\n")) {
    if (!line.trim()) continue;
    let j: Record<string, unknown>;
    try {
      j = JSON.parse(line);
    } catch {
      continue;
    }
    const type = j.type as string | undefined;
    if (type === "assistant") {
      const msg = j.message as { usage?: Record<string, number> } | undefined;
      const u = msg?.usage;
      if (u) {
        inputTokens += u.input_tokens ?? 0;
        cacheCreationTokens += u.cache_creation_input_tokens ?? 0;
        cacheReadTokens += u.cache_read_input_tokens ?? 0;
        outputTokens += u.output_tokens ?? 0;
        numTurns += 1;
      }
    } else if (type === "result") {
      costUsd = (j.total_cost_usd as number) ?? 0;
      answer = (j.result as string) ?? "";
      isError = (j.is_error as boolean) ?? false;
      // num_turns in the final result is authoritative; fall back to our count
      const nt = j.num_turns as number | undefined;
      if (typeof nt === "number") numTurns = nt;
    }
  }

  return {
    ok: !isError,
    wallMs,
    inputTokens: inputTokens + cacheCreationTokens + cacheReadTokens,
    cacheReadTokens,
    outputTokens,
    costUsd,
    numTurns,
    answer,
    error: isError ? answer : undefined,
  };
}

// ──────────────────────────────────────────────────────────────────────────
// Main loop
// ──────────────────────────────────────────────────────────────────────────

interface PairResult {
  question: string;
  baseline: RunResult;
  withCtx: RunResult;
}

type Label = "baseline" | "with-ctx";
interface Job {
  idx: number;
  label: Label;
  mcpCfg: string;
  question: string;
}

const jobs: Job[] = [];
for (let i = 0; i < questions.length; i++) {
  jobs.push({ idx: i, label: "baseline", mcpCfg: baselineCfg, question: questions[i]! });
  jobs.push({ idx: i, label: "with-ctx", mcpCfg: withCtxCfg, question: questions[i]! });
}

console.error(
  `\nrunning ${jobs.length} jobs (${questions.length} questions × 2 legs) at concurrency=${concurrency}\n`,
);

// Pool: run up to `concurrency` jobs at once. Each slot pulls the next job
// when it finishes.
const jobResults = new Array<RunResult | null>(jobs.length).fill(null);
let nextJob = 0;
let finished = 0;
const totalJobs = jobs.length;

async function worker(workerId: number): Promise<void> {
  while (true) {
    const jobIdx = nextJob;
    if (jobIdx >= jobs.length) return;
    nextJob = jobIdx + 1;
    const job = jobs[jobIdx]!;
    const tag = `[${job.idx + 1}${job.label === "baseline" ? "a" : "b"}/${totalJobs}]`;
    console.error(`${tag} [w${workerId}] start ${job.label}`);
    const start = performance.now();
    const res = await runOne(job.mcpCfg, job.question);
    const dt = Math.round(performance.now() - start);
    jobResults[jobIdx] = res;
    finished += 1;
    console.error(
      `${tag} [w${workerId}] ${res.ok ? "ok" : "FAIL"}  ${dt}ms  ` +
        `$${res.costUsd.toFixed(4)}  turns=${res.numTurns}  out=${res.outputTokens}  ` +
        `(${finished}/${totalJobs} done)`,
    );
  }
}

const workers: Promise<void>[] = [];
for (let w = 0; w < Math.min(concurrency, totalJobs); w++) {
  workers.push(worker(w));
}
await Promise.all(workers);

// Reassemble results keyed by question index
const results: PairResult[] = [];
for (let i = 0; i < questions.length; i++) {
  const baselineJobIdx = jobs.findIndex(
    (j) => j.idx === i && j.label === "baseline",
  );
  const withCtxJobIdx = jobs.findIndex(
    (j) => j.idx === i && j.label === "with-ctx",
  );
  results.push({
    question: questions[i]!,
    baseline: jobResults[baselineJobIdx]!,
    withCtx: jobResults[withCtxJobIdx]!,
  });
}

// ──────────────────────────────────────────────────────────────────────────
// Report
// ──────────────────────────────────────────────────────────────────────────

function fmtMs(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

function fmtDelta(a: number, b: number): string {
  // Positive = ctx is better (smaller)
  if (a === 0) return "∞";
  const ratio = a / b;
  if (ratio >= 1) return `${ratio.toFixed(2)}× less`;
  return `${(1 / ratio).toFixed(2)}× MORE`;
}

console.log("\n## Benchmark results\n");
console.log(`Repo: \`${repoDir}\`  ·  Questions: ${questions.length}\n`);

console.log(
  "| # | Question | Time | In tokens | Out tokens | Cost |",
);
console.log(
  "|---|----------|------|-----------|------------|------|",
);

for (let i = 0; i < results.length; i++) {
  const r = results[i]!;
  const qShort =
    r.question.length > 60
      ? r.question.slice(0, 57) + "…"
      : r.question;
  console.log(
    `| ${i + 1}a baseline | ${qShort.replace(/\|/g, "\\|")} | ${fmtMs(r.baseline.wallMs)} | ${r.baseline.inputTokens.toLocaleString()} | ${r.baseline.outputTokens.toLocaleString()} | $${r.baseline.costUsd.toFixed(4)} |`,
  );
  console.log(
    `| ${i + 1}b with-ctx | ↑ | ${fmtMs(r.withCtx.wallMs)} | ${r.withCtx.inputTokens.toLocaleString()} | ${r.withCtx.outputTokens.toLocaleString()} | $${r.withCtx.costUsd.toFixed(4)} |`,
  );
  console.log(
    `| Δ | | ${fmtDelta(r.baseline.wallMs, r.withCtx.wallMs)} | ${fmtDelta(r.baseline.inputTokens, r.withCtx.inputTokens)} | | ${fmtDelta(r.baseline.costUsd, r.withCtx.costUsd)} |`,
  );
}

// Aggregate
const sum = (f: (r: PairResult) => number) =>
  results.reduce((a, r) => a + f(r), 0);

const totB = sum((r) => r.baseline.wallMs);
const totC = sum((r) => r.withCtx.wallMs);
const inB = sum((r) => r.baseline.inputTokens);
const inC = sum((r) => r.withCtx.inputTokens);
const outB = sum((r) => r.baseline.outputTokens);
const outC = sum((r) => r.withCtx.outputTokens);
const costB = sum((r) => r.baseline.costUsd);
const costC = sum((r) => r.withCtx.costUsd);

console.log("\n## Totals\n");
console.log(
  "| Metric | Baseline | With ctx | Δ |",
);
console.log(
  "|--------|----------|----------|---|",
);
console.log(
  `| Wall clock | ${fmtMs(totB)} | ${fmtMs(totC)} | ${fmtDelta(totB, totC)} |`,
);
console.log(
  `| Input tokens | ${inB.toLocaleString()} | ${inC.toLocaleString()} | ${fmtDelta(inB, inC)} |`,
);
console.log(
  `| Output tokens | ${outB.toLocaleString()} | ${outC.toLocaleString()} | ${fmtDelta(outB, outC)} |`,
);
console.log(
  `| Cost (USD) | $${costB.toFixed(4)} | $${costC.toFixed(4)} | ${fmtDelta(costB, costC)} |`,
);

// Exit non-zero if any run failed, so CI can gate on it
const anyFail = results.some((r) => !r.baseline.ok || !r.withCtx.ok);
process.exit(anyFail ? 1 : 0);

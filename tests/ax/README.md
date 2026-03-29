# AX Test Harness

Agent Experience (AX) quality assessment for gaal. Three layers that measure how well gaal teaches agents to use it correctly.

## Quick Start

```bash
# Layer 1 only — free, no API calls
./run-ax.sh --layer 1 --skip-judge

# Full dry run — shows what would be dispatched
./run-ax.sh --dry-run

# Full run — costs API tokens via agent-mux
./run-ax.sh
```

## Architecture

### Layer 1: Error Quality Audit (`layer1-errors/`)

Triggers every known gaal error path and evaluates message quality.

- **`error-manifest.toml`** — Declarative list of error-triggering commands. Source of truth. Add entries here when adding new commands or flags to gaal.
- **`generate-errors.sh`** — Runs every command from the manifest, captures stdout/stderr/exit code, outputs `errors.json`.
- **`judge-errors.sh`** — Dispatches errors.json to a Codex gpt-5.4 xhigh worker that rates each error: TEACHES / HINTS / CRYPTIC.

**Cost:** `generate-errors.sh` is free (runs gaal locally). `judge-errors.sh` costs one Codex dispatch.

### Layer 2: First-Attempt Agent Tasks (`layer2-tasks/`)

Tests whether agents can accomplish real gaal tasks on their first attempt with only SKILL.md and docs/.

- **`tasks.toml`** — Declarative task definitions with prompts, expected command patterns, and agent count per task.
- **`run-tasks.sh`** — Dispatches agents via agent-mux, collects responses and session metadata.

**Cost:** 3 agents x 12 tasks = 36 Codex dispatches (configurable via `--agents`).

### Layer 3: Trace Analysis (`layer3-analysis/`)

Synthesizes findings from Layers 1 and 2 into actionable insights.

- **`analyze-traces.sh`** — Dispatches a single Codex gpt-5.4 xhigh worker to analyze convergence, variance, error recovery, implicit API, and cost efficiency.

**Cost:** One Codex dispatch.

## Adding New Tests

### New error path

Add an entry to `layer1-errors/error-manifest.toml`:

```toml
[[errors]]
name = "my-new-error"
command = "gaal newcmd --bad-flag"
expected_exit = 11
description = "What this tests"
```

Then run `./run-ax.sh --layer 1 --skip-judge` to verify the exit code matches.

### New agent task

Add an entry to `layer2-tasks/tasks.toml`:

```toml
[[tasks]]
name = "my-new-task"
prompt = "The natural-language task description"
expected_command_pattern = "gaal.*expected.*regex"
agents_per_task = 3
category = "category-name"
```

## Output Artifacts

| File | Layer | What |
|------|-------|------|
| `layer1-errors/errors.json` | 1 | Raw error outputs with exit code validation |
| `layer1-errors/judgments.json` | 1 | AX quality ratings per error (TEACHES/HINTS/CRYPTIC) |
| `layer2-tasks/results.json` | 2 | Per-agent, per-task dispatch results and session metadata |
| `layer3-analysis/analysis.json` | 3 | Structured analysis (convergence, variance, recovery, cost) |
| `layer3-analysis/analysis.md` | 3 | Human-readable AX quality report |

## Exit Codes Reference

| Code | Meaning | Test Coverage |
|------|---------|---------------|
| 0 | Success | salt, tag ls, index status, recall (no args), transcript (no args) |
| 1 | No results | ls empty window, search miss, who miss, recall miss |
| 2 | Ambiguous ID | inspect/transcript with 1-char prefix |
| 3 | Not found | inspect/transcript/tag/reindex/handoff/find-salt with bad ID |
| 10 | Missing index | Not tested (requires deleting the index) |
| 11 | Parse error | who (no verb, bad verb), search (empty), find-salt (no arg), tag (no args), ls (bad --since), inspect (no selector) |

## Dependencies

- `gaal` binary on PATH (built with `cargo build --release`)
- `agent-mux` binary on PATH
- `jq` for JSON processing
- Indexed sessions (`gaal index backfill`)

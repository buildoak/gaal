# `gaal create-handoff`

Purpose: generate handoff markdown via LLM extraction, either for one session or in batch.

Handoffs are optional continuity artifacts. Core Gaal workflows - indexing, fleet views, inspect, transcript rendering, search, attribution, and tags - work without any LLM backend. `create-handoff` uses `agent-mux` by default and can spend tokens when not run with `--dry-run`.

## Usage

```bash
gaal create-handoff [OPTIONS] [ID]
```

## Flags

- `--jsonl <path>`: explicit JSONL override
- `--engine <claude|codex|gemini|hermes>`: extraction engine override
- `--model <model>`
- `--prompt <path>`
- `--provider <agent-mux|openrouter>`: provider selector; default `agent-mux`.
  `agent-mux` is the supported real-execution provider. `openrouter` may be
  visible for planning/dry-run compatibility, but real execution is not
  implemented unless dry-run reports `provider_supported: true`.
- `--format <string>`: default `eywa-compatible`
- `--batch`
- `--since <duration|date>`: default `7d`
- `--parallel <n>`: default `1`
- `--min-turns <n>`: default `3`
- `--this`: prefer the current detected session rather than a parent
- `--dry-run`: preview candidates/planned execution only. Single-session dry-run is read-only: it does not invoke providers, spend tokens, write handoff markdown, upsert handoff rows, or index unindexed JSONL files.
- `--effort <low|medium|high|xhigh>`: effort level for the default `agent-mux` dispatch path. Overrides config `[agent-mux] effort`. Controls how long the LLM worker runs and auto-aligns gaal's wrapper timeout.
- `-H`, `--human`

## ID Resolution

`ID` may be:

- a session ID
- `today`
- `latest`

`latest` resolves to the most recent session.

## JSON Output

Single-session mode returns an array of handoff results with `session_id`, `handoff_path`, `headline`, `projects`, `keywords`, and `substance`.

Batch mode returns per-session status rows.

`--dry-run` still returns JSON rows. For single-session mode, each row is a planning record with:

- `strategy`: `single`, `chunked_compaction`, or `chunked_turn_split`
- `estimated_transcript_chars`, `estimated_transcript_tokens`
- `compaction_lines`
- `chunk_count`, `estimated_llm_calls`
- `provider`, `provider_supported`, `model`, `effort`, `format`
- `handoff_path`
- `side_effects`: all false in dry-run
- `warnings`

For unindexed `--jsonl --dry-run`, Gaal reads the JSONL path directly, reports `indexed=false`, and does not index it.

Provider note: `agent-mux` is optional for Gaal itself but is the default backend for real handoff generation. Verify it before non-dry-run handoffs, especially batch handoffs. `--dry-run` is the safest way to inspect planned provider/model/effort choices without side effects.

When single-session planning returns a strategy other than `single`, non-dry-run execution uses automatic chunked generation: mapper calls process each planned chunk, one reducer call synthesizes the final handoff, and Gaal writes only the final markdown file plus the normal handoff DB row. Mapper outputs stay in memory and no surfaced `.parts` files are created. The final markdown includes a `Coverage Manifest` reporting source JSONL line ranges, source used, chunk statuses, mapper/reducer call counts, and whether rendered transcript line ranges were approximate.

## Real Example

```bash
$ gaal create-handoff --batch --dry-run --since 1d --min-turns 3
[
  {
    "session_id": "aed14881",
    "status": "pending",
    "handoff_path": null,
    "error": null,
    "duration_secs": 0.0
  },
  {
    "session_id": "a7e8c6f6",
    "status": "pending",
    "handoff_path": null,
    "error": null,
    "duration_secs": 0.0
  }
]
```

## Self-Handoff

For self-handoff from a running agent session, use the separate salt-discovery flow documented in [`gaal salt` / `gaal find-salt`](./self-id.md).

## Related Commands

- [`gaal transcript`](./drill-down.md)
- [`gaal recall`](./search-recall.md)
- [`gaal salt`](./self-id.md)

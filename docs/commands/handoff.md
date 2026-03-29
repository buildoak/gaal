# `gaal create-handoff`

Purpose: generate handoff markdown via LLM extraction, either for one session or in batch.

## Usage

```bash
gaal create-handoff [OPTIONS] [ID]
```

## Flags

- `--jsonl <path>`: explicit JSONL override
- `--engine <claude|codex>`: extraction engine override
- `--model <model>`
- `--prompt <path>`
- `--provider <agent-mux|openrouter>`: default `agent-mux`
- `--format <string>`: default `eywa-compatible`
- `--batch`
- `--since <duration|date>`: default `7d`
- `--parallel <n>`: default `1`
- `--min-turns <n>`: default `3`
- `--this`: prefer the current detected session rather than a parent
- `--dry-run`: preview candidates only
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

`--dry-run` still returns JSON rows, with candidate summary lines printed to stderr.

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

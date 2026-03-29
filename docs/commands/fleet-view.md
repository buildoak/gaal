# `gaal ls`

Purpose: fleet view over indexed sessions.

## Usage

```bash
gaal ls [OPTIONS]
```

## Flags

- `--engine <claude|codex>`
- `--since <duration|date>`
- `--before <date|timestamp>`
- `--cwd <substring>`
- `--tag <tag>`: repeatable, AND semantics
- `--session-type <coordinator|standalone|subagent>`
- `--sort <started|ended|tokens|cost|duration>`
- `--limit <n>`: default `10`
- `--aggregate`: return totals instead of session rows
- `--all`: include short/noise sessions
- `--skip-subagents`: hide subagent rows
- `-H`, `--human`

## JSON Output

Default output is an envelope with:

- `query_window`
- optional `filter`
- `shown`
- `total`
- optional `total_unfiltered`
- `sessions`

Each `sessions` row includes the indexed session summary fields shown in the example below, including `id`, `engine`, `model`, `cwd`, timestamps, duration, token totals, `peak_context`, `tools_used`, `headline`, and `session_type`.

## Aggregate Mode

`--aggregate` switches from per-session rows to totals. Aggregate mode returns totals plus engine buckets instead of a `sessions` list.

## Human Output

Human mode prints a table with task/headline, engine, start time, duration, token totals, peak context, tools, model, and cwd.

## Real Examples

```bash
$ gaal ls --limit 2
{
  "query_window": {
    "from": "2026-01-08",
    "to": "2026-03-29T10:47:09Z"
  },
  "filter": "hiding sessions with 0 tool calls and <30s duration",
  "shown": 2,
  "total": 2,
  "total_unfiltered": 7202,
  "sessions": [
    {
      "id": "ab3f2e83",
      "engine": "codex",
      "model": "gpt-5.4",
      "cwd": "gaal",
      "started_at": "2026-03-29T10:46:13.988Z",
      "ended_at": "2026-03-29T10:46:56.904Z",
      "duration_secs": 42,
      "tokens": { "input": 52262, "output": 1653 },
      "peak_context": 48353,
      "tools_used": 14,
      "headline": "# Lifter Deep You are a disciplined builder for hard prob...",
      "session_type": "standalone"
    }
  ]
}
```

```bash
$ gaal ls --limit 2 -H
ID        Task               Engine  Started      Duration  Tokens     Peak  Tools  Model    CWD
--------  -----------------  ------  -----------  --------  ---------  ----  -----  -------  ----
ab3f2e83  # Lifter Deep ...  codex   today 14:46  42s       52K / 1K   48K   14     gpt-5.4  gaal
```

## Related Commands

- [`gaal inspect`](./drill-down.md)
- [`gaal tag`](./index-tags.md)

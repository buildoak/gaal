# `gaal activity`

Purpose: render source-backed historical activity slices across sessions for a time window.

`activity` is not live process monitoring. It reads indexed session candidates, parses their raw source traces, and renders transcript-shaped markdown for events proven to overlap the requested window.

## Usage

```bash
gaal activity [OPTIONS]
```

Common flags:

- `--since <duration|date|timestamp>`: lower bound; default `1d`
- `--before <date|timestamp>`: upper bound; default now
- `--engine <claude|codex|gemini|agy|hermes|grok>`
- `--cwd <substring>`
- `--session <reference>`: render one resolved session only
- `--skip-subagents`
- `--limit <n>`: max DB candidates
- `--stdout`: print markdown instead of path metadata
- `--force`: accepted for cache parity; activity output is regenerated
- `-H`, `--human`

Windows are half-open: `[since,before)`.

## Examples

```bash
gaal activity --since 1d -H
gaal activity --since 2026-05-25 --before 2026-05-26 --stdout
gaal activity --session 9ad81c91 --since 2026-05-25 --before 2026-05-26
gaal activity --engine codex --since 7d
```

## Output

Default output is JSON with:

- `path`, `size_bytes`, `estimated_tokens`, `warning`
- `query_window`
- `sessions_rendered`
- `skipped`
- `degraded`

`--stdout` prints the markdown bundle. Each included session slice keeps the normal transcript shape and adds activity frontmatter such as `render_kind`, `slice_since`, `slice_before`, `carried_in`, and `continues_after`.

Use `gaal ls` to find sessions and `gaal inspect <id>` for per-session structured detail.

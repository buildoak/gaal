# Eywa to gaal Migration

`gaal` replaces `eywa` for session recall and handoff generation. This page covers the one-time migration steps and ongoing command mapping.

## Command Mapping

| Eywa command | Gaal equivalent |
|-------------|----------------|
| `eywa get` | `gaal recall --format eywa` |
| `eywa get "query"` | `gaal recall "query" --format eywa` |
| `eywa get "topic" --days-back 30 --max 5` | `gaal recall "topic" --days-back 30 --limit 5 --format eywa` |
| `eywa extract` | `gaal create-handoff` |
| `eywa extract <id>` | `gaal create-handoff <id>` |
| `eywa rebuild-index` | `gaal index backfill` |

## Importing Eywa Data

One-time import of legacy `eywa` handoff-index data:

```bash
gaal index import-eywa [PATH]
```

Output: JSON summary with `imported`, `skipped`, and `errors` counts.

If `PATH` is not specified, `gaal` looks in the default `eywa` data directory.

## The `--format eywa` Flag

The `--format eywa` flag on `gaal recall` produces coordinator-compatible output matching `eywa`'s original layout. Use this during the transition period when consuming agents still expect `eywa`-shaped responses.

For new integrations, prefer `--format brief` (default) or `--format summary`.

## What Changed

Key differences between `eywa` and `gaal`:

- `gaal` indexes Claude, Codex, and Gemini sessions (`eywa` was Claude-only)
- `gaal` uses SQLite + Tantivy instead of `eywa`'s flat-file approach
- Handoff generation uses `agent-mux` by default (`eywa` used direct LLM calls)
- `gaal`'s recall is BM25-ranked over handoff content, more accurate than `eywa`'s keyword matching
- `gaal` adds attribution (`who`), search, transcript, tagging, and self-identification that `eywa` never had

## Post-Migration

After importing `eywa` data:

1. Verify with `gaal index status` (check `handoffs_total` includes imported count)
2. Test recall: `gaal recall "topic" --format eywa --limit 3`
3. Update any scripts or agent configs that reference `eywa` commands
4. The `eywa` binary can be removed once all consumers are migrated

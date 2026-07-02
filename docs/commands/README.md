# Command Reference

Gaal commands fall into four practical groups: discovery, reading, continuity,
and maintenance. Default output is JSON for agent use; add `-H` when a human
table or card is more useful.

## Discovery

| Command | Use |
| --- | --- |
| [`gaal ls`](fleet-view.md) | List indexed sessions, filter by engine/type/date/cwd/tag, and aggregate totals. |
| [`gaal search`](search-recall.md) | Full-text search over indexed facts. |
| [`gaal who`](attribution.md) | Find sessions that read, wrote, ran, changed, touched, or deleted a target. |
| [`gaal resolve`](self-id.md) | Resolve a short ID, prefix, or Hermes alias to source and artifact paths. |

## Reading

| Command | Use |
| --- | --- |
| [`gaal inspect`](drill-down.md) | Inspect one or more sessions with focused views for files, commands, errors, tokens, or trace metadata. |
| [`gaal transcript`](drill-down.md) | Locate or print a deterministic markdown transcript for a session. |
| [`gaal activity`](activity.md) | Render source-backed activity slices across a time window. |

## Continuity

| Command | Use |
| --- | --- |
| [`gaal recall`](search-recall.md) | Retrieve generated handoffs by topic or direct session ID. |
| [`gaal create-handoff`](handoff.md) | Generate optional continuity markdown from a session. Use `--dry-run` first. |
| [`gaal salt` / `gaal find-salt`](self-id.md) | Identify the current Claude Code, Codex, or Antigravity session from inside an agent run. |

## Maintenance

| Command | Use |
| --- | --- |
| [`gaal index`](index-tags.md) | Backfill, inspect, reindex, prune, or recover derived index state. |
| [`gaal tag`](index-tags.md) | Add, remove, or list local session tags. |

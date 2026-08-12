# Codex Fixtures

This fixture tree is a sanitized fake `HOME` for Gaal codex integration tests.
It intentionally contains no real Codex rollout content.

Layout covered:

- `.codex/sessions/2026/05/05/rollout-<timestamp>-<uuid>.jsonl`

Codex session IDs are the last 8 hex characters of the dash-stripped UUID:

- `01960000-0000-7000-8000-0000cafe0001` -> `cafe0001` (coordinator)
- `01960000-0000-7000-8000-0000cafe0002` -> `cafe0002` (subagent of `cafe0001`)
- `01960000-0000-7000-8000-0000cafe0003` -> `cafe0003` (subagent of `cafe0001`)
- `01960000-0000-7000-8000-0000cafe0004` -> `cafe0004` (subagent of `dead0000`)

The coordinator starts before its subagents, so newest-first discovery reaches
the children first. That is the ordering that used to break `sessions.parent_id`
with `FOREIGN KEY constraint failed`.

`cafe0004` forks from `01960000-0000-7000-8000-0000dead0000`, which has no
rollout file. It covers the parent that never gets indexed at all.

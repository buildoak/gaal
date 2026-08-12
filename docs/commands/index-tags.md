# `gaal index`

Purpose: index maintenance and corpus mutation commands.

## Usage

```bash
gaal index <SUBCOMMAND> [OPTIONS]
```

## `index backfill`

Usage:

```bash
gaal index backfill [OPTIONS]
```

Flags:

- `--engine <claude|codex|gemini|agy|hermes|grok>`
- `--since <date|timestamp>`
- `--force`
- `--with-markdown`
- `--output-dir <path>`: implies `--with-markdown`
- `-H`, `--human`

Output:

- JSON summary: `indexed`, `skipped`, `errors`, optional `markdown_written`, optional `markdown_skipped`
- Progress lines go to stderr during the run

### Incremental behavior

`index backfill` is incremental. An unfiltered run includes every supported
engine, including Grok; `--engine` narrows that run. Each engine keeps its own
mtime cursor in the `meta` SQLite table, for example `backfill:claude`,
`backfill:codex`, `backfill:gemini`, `backfill:agy`, `backfill:hermes`, and
`backfill:grok`. On every run, discovery for an engine skips source artifacts
whose on-disk mtime is older than `cursor - 10s` before parsing or SQLite lookup.
The 10-second safety margin covers actively changing session artifacts whose
mtime is close to wall-clock.

Cursor advancement rules:

- A cursor advances only when that engine's pass completes successfully.
- If one engine stalls, its cursor stays put and the next run retries the missed window. Other engines still advance independently.
- First run (no cursor) and DB wipes fall through to a full scan — the cursor is absent, so no mtime gate applies.
- `--engine`, `--since`, and `--force` are all additive on top of the mtime gate. `--engine claude` only advances the Claude cursor. `--since` narrows further. `--force` re-indexes already-known rows but still honors the mtime gate for discovery.

### Parent links

Codex subagents and Hermes forks record the session they came from.
`sessions.parent_id` is a foreign key, so those links can only be written once
the parent row exists. Discovery is newest-first and a child always starts after
its parent, so discovery sorts linked sessions to the back of the pass and the
indexer fills any link it could not resolve inline after the pass completes.

A parent that is never indexed — its trace was deleted, or it sits outside the
run's window — leaves the child indexed, typed as a subagent, and unlinked. That
case prints one stderr line per session:

```text
  -> session 9767dfaf: parent 4105a92a is not indexed; link left unset
```

Re-running backfill over a window that includes the parent restores the link.

### Agy discovery behavior

Agy discovery scans Antigravity brain directories, prefers non-empty
`transcript_full.jsonl`, and falls back to `transcript.jsonl`. Native agy IDs
are the first 8 characters of the brain UUID. JSONL parsing also detects copied
agy transcripts by record shape. Outside the native brain path, copied agy files
preserve engine identity; session identity comes from explicit content IDs when
present, otherwise Gaal falls back to the copied filename.

Optional agent-mux sidecars can fill model or missing cwd for matching agy
sessions. Indexing does not require agent-mux.

### Grok discovery behavior

Grok discovery scans `${GROK_HOME:-~/.grok}/sessions/<encoded-cwd>/<uuid>/`.
Each directory is one canonical full-UUID session; `updates.jsonl` is the
primary visible event source and `chat_history.jsonl` is fallback. Grok is part
of the default unfiltered pass. Use `--engine grok` only for a Grok-scoped
diagnostic, repair, or test run.

### Troubleshooting: stuck cursor

If a backfill cursor ever gets stuck (e.g., an engine that keeps failing partway through and you want to force it to re-evaluate everything), delete the cursor rows and rerun:

```bash
sqlite3 ~/.gaal/index.db "DELETE FROM meta WHERE key LIKE 'backfill:%'"
gaal index backfill
```

This clears the per-engine cursors without touching indexed sessions or facts. The next run performs a full-scan baseline and rewrites the cursors on success.

## `index status`

Usage:

```bash
gaal index status
```

Flags:

- `-H`, `--human`

Real example:

```bash
$ gaal index status
{
  "db_path": "/home/alex/.gaal/index.db",
  "db_size_bytes": 387366912,
  "facts_total": 249747,
  "grok": {
    "malformed_records": 0,
    "parser_observations": 0,
    "private_records": 0,
    "redacted_records": 0,
    "sessions_with_artifacts": 0,
    "source_artifacts": 0,
    "unknown_records": 0
  },
  "handoffs_total": 871,
  "last_indexed_at": "2026-03-29T10:46:56.904Z",
  "newest_session": "2026-03-29T10:46:13.988Z",
  "oldest_session": "2026-01-08",
  "sessions_by_engine": { "claude": 4277, "codex": 2925, "agy": 12 },
  "sessions_total": 7214
}
```

The `grok` object is always present. Its counters expose source-artifact and
parser-drift diagnostics without surfacing private record bodies. Human output
shows the Grok diagnostics section when artifact or observation rows exist.

## `index reindex`

Usage:

```bash
gaal index reindex <ID>
```

Output:

- JSON summary with `session_id` and `facts`

## `index prune`

Usage:

```bash
gaal index prune --before <DATE>
```

Flags:

- `--before <date>`
- `-H`, `--human`

Output:

- JSON object with `before` and `deleted`

## `index recover-orphans`

One-off recovery for subagent JSONL files orphaned when Claude Code's 30-day cleanup deletes parent session files. Scans `~/.claude/projects/` for subagent files whose parent session is missing or unlinked, creates ghost parent records tagged `_recovered`, and indexes the orphaned subagents.

Usage:

```bash
gaal index recover-orphans [--dry-run]
```

Flags:

- `--dry-run`: preview orphan counts without writing to the database

Output:

- Dry-run: `{ "orphan_files": N, "parent_groups": N, "dry_run": true }`
- Live: `{ "ghosts_created": N, "subagents_indexed": N, "errors": N }`

Run with `--dry-run` first. This is an admin-only recovery tool, not part of normal workflow.

## Operational Note

Any command that mutates facts rebuilds the Tantivy index afterwards. Rebuild triggers include `gaal index backfill` (only when at least one engine indexed new sessions), `gaal index reindex`, `gaal index prune`, and `gaal index recover-orphans`.

# `gaal tag`

Purpose: add, remove, or list session tags.

## Usage

```bash
gaal tag [OPTIONS] [ID] [TAGS]...
```

## Flags

- `--remove`: remove tags instead of adding them
- `-H`, `--human`

## Modes

- `gaal tag ls`: list all distinct tags
- `gaal tag <id> <tag1> <tag2>`: add tags
- `gaal tag <id> <tag1> --remove`: remove tags

## Output

- `tag ls` returns a JSON array of strings
- tag mutations return `{ "session_id": "...", "action": "added|removed", "tags": [...] }`

## Real Example

```bash
$ gaal tag ls
[
  "bot",
  "build-gaal",
  "coordinator",
  "legacy",
  "test-tag",
  "worker"
]
```

## Related Commands

- [`gaal ls`](./fleet-view.md)
- [`gaal inspect`](./drill-down.md)

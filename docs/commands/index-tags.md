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

- `--engine <claude|codex>`
- `--since <date|timestamp>`
- `--force`
- `--with-markdown`
- `--output-dir <path>`: implies `--with-markdown`
- `-H`, `--human`

Output:

- JSON summary: `indexed`, `skipped`, `errors`, optional `markdown_written`, optional `markdown_skipped`
- Progress lines go to stderr during the run

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
  "db_path": "/Users/otonashi/.gaal/index.db",
  "db_size_bytes": 387366912,
  "facts_total": 249747,
  "handoffs_total": 871,
  "last_indexed_at": "2026-03-29T10:46:56.904Z",
  "newest_session": "2026-03-29T10:46:13.988Z",
  "oldest_session": "2026-01-08",
  "sessions_by_engine": { "claude": 4277, "codex": 2925 },
  "sessions_total": 7202
}
```

## `index reindex`

Usage:

```bash
gaal index reindex <ID>
```

Output:

- JSON summary with `session_id` and `facts`

## `index import-eywa`

Usage:

```bash
gaal index import-eywa [PATH]
```

Output:

- JSON summary with `imported`, `skipped`, and `errors`

Detailed migration guidance lives in `migration.md`.

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

## Operational Note

Any command that mutates facts rebuilds the Tantivy index afterwards. Rebuild triggers include `gaal index backfill`, `gaal index reindex`, `gaal index prune`, and `gaal index import-eywa`.

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
- add/remove returns `{ "session_id": "...", "action": "added|removed", "tags": [...] }`

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

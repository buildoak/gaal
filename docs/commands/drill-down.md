# `gaal inspect`

Purpose: session detail view for one or more sessions. This is the replacement for the older `show` command.

## Usage

```bash
gaal inspect [OPTIONS] [ID]
```

## Flags

- `--files [read|write|all]`: include file operations; bare `--files` defaults to `all`
- `--errors`: include errors and non-zero exits only
- `--commands`: include shell command entries
- `--git`: include git operations
- `-F`, `--full`: include all arrays and fields
- `--tokens`: include token breakdown
- `--trace`: include full fact timeline
- `--source`: include raw JSONL path
- `--include-empty`: keep low-signal subagents in coordinator views
- `--ids <id1,id2,...>`: batch mode by comma-delimited prefixes
- `--tag <tag>`: batch mode by tag
- `-H`, `--human`: human-readable output

## Output

JSON:

- Default output is a compact session card with counts, token summary, tags, and `session_type`.
- Focused flags swap in specific payloads such as `files`, `commands`, `errors`, `git_ops`, or `trace`.
- Batch mode returns an array.

Human:

- Card view with engine, model, duration, cwd, files, ops, and token notes.
- Coordinators include a subagent table.

## Real Example

```bash
$ gaal inspect latest
{
  "command_count": 11,
  "cwd": "/Users/otonashi/thinking/building/gaal",
  "duration_secs": 42,
  "ended_at": "2026-03-29T10:46:56.904Z",
  "engine": "codex",
  "error_count": 0,
  "file_count": { "edited": 0, "read": 0, "written": 0 },
  "git_op_count": 0,
  "id": "ab3f2e83",
  "model": "gpt-5.4",
  "peak_context": 48353,
  "session_type": "standalone",
  "started_at": "2026-03-29T10:46:13.988Z",
  "tags": [],
  "tokens": { "input": 52262, "output": 1653 },
  "tools_used": 14,
  "turns": 1
}
```

# `gaal transcript`

Purpose: return or render the markdown transcript for a session.

## Usage

```bash
gaal transcript [OPTIONS] [ID]
```

## Flags

- `--force`: re-render even if cached markdown already exists
- `--stdout`: print markdown instead of JSON path metadata
- `-H`, `--human`

## Behavior

- Default mode resolves or renders the transcript file and returns path metadata.
- `--stdout` prints the markdown body.
- If no ID is provided, the command prints help and exits successfully.

## Output

JSON fields:

- `path`
- `size_bytes`
- `estimated_tokens`
- `warning`

Human:

- Three-line summary with path, size, estimated tokens, and warning.

## Real Example

```bash
$ gaal transcript latest
{
  "path": "/Users/otonashi/.gaal/data/codex/sessions/2026/03/29/ab3f2e83.md",
  "size_bytes": 16034,
  "estimated_tokens": 4008,
  "warning": "~4K tokens. Recommend reading via subagent, not coordinator context."
}
```

## Related Commands

- [`gaal ls`](./fleet-view.md)
- [`gaal who`](./attribution.md)
- [`gaal create-handoff`](./handoff.md)

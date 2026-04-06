# `gaal salt`

Purpose: generate a unique salt token for content-addressed self-identification.

## Usage

```bash
gaal salt
```

## Output Format

Raw token string on stdout, not JSON.

Format:

```text
GAAL_SALT_<16 hex chars>
```

## Real Example

```bash
$ gaal salt
GAAL_SALT_d0a6e1d5530bf6c9
```

# `gaal find-salt`

Purpose: scan Claude, Codex, and Gemini session logs and return the first file containing a salt token. Returns enriched session context when the session is indexed, so agents can self-identify in a single call without chaining `inspect`/`transcript`/`recall`.

## Usage

```bash
gaal find-salt [OPTIONS] [SALT]
```

## Flags

- `-H`, `--human`

## JSON Output

When the session is indexed (has been processed by `gaal index backfill`):

- `session_id` — raw filename-derived session identifier
- `engine` — `claude`, `codex`, or `gemini`
- `jsonl_path` — absolute path to the JSONL file
- `indexed` — `true`
- `model` — model name (e.g. `claude-opus-4-6`)
- `cwd` — working directory of the session
- `session_type` — `standalone`, `coordinator`, or `subagent`
- `last_event_at` — timestamp of most recent event
- `turns` — total conversation turns
- `total_tokens` — combined input + output tokens
- `input_tokens` — input tokens only
- `output_tokens` — output tokens only
- `transcript_path` — expected path to rendered transcript markdown
- `transcript_exists` — whether the transcript file exists on disk
- `handoff.exists` — whether a handoff has been generated
- `handoff.generated_at` — handoff generation timestamp (if exists)

When not indexed:

- `session_id`, `engine`, `jsonl_path` — same as above
- `indexed` — `false`

Notes:

- The returned `session_id` is derived from the JSONL filename stem, so Codex and Claude shapes differ.
- This command scans `~/.claude/projects/`, `~/.codex/`, and `~/.gemini/tmp/`.
- Enrichment is best-effort: if the DB is unavailable or the session is not indexed, the command still succeeds with the base 3 fields plus `"indexed": false`.

## Real Examples

Enriched output (indexed session):

```bash
$ gaal find-salt GAAL_SALT_40e4d9ceb25e0dd1
{"cwd":"/Users/otonashi/thinking/pratchett-os/coordinator","engine":"claude","handoff":{"exists":true,"generated_at":"2026-03-27T09:45:04Z"},"indexed":true,"input_tokens":883,"jsonl_path":"/Users/otonashi/.claude/projects/.../5e54db27-a30e-455c-af24-26a3c55e511e.jsonl","last_event_at":"2026-03-27T09:45:20Z","model":"claude-opus-4-6","output_tokens":1400,"session_id":"5e54db27-a30e-455c-af24-26a3c55e511e","session_type":"coordinator","total_tokens":2283,"transcript_exists":true,"transcript_path":"/Users/otonashi/.gaal/data/claude/sessions/2026/03/27/5e54db27.md","turns":31}
```

Human-readable output (`-H`):

```bash
$ gaal find-salt GAAL_SALT_40e4d9ceb25e0dd1 -H
Session: 5e54db27-a30e-455c-af24-26a3c55e511e
Engine:  claude (claude-opus-4-6)
Type:    coordinator
CWD:     /Users/otonashi/thinking/pratchett-os/coordinator
Tokens:  2K (883 in / 1K out) | 31 turns
Last:    2026-03-27T09:45:20.375Z
JSONL:   /Users/otonashi/.claude/projects/.../5e54db27-a30e-455c-af24-26a3c55e511e.jsonl
Transcript: /Users/otonashi/.gaal/data/claude/sessions/2026/03/27/5e54db27.md
Handoff: yes (generated 2026-03-27T09:45:04Z)
```

Non-indexed session (`-H`):

```
Session: abc12345-defg-...
Engine:  claude
JSONL:   /path/to/session.jsonl
Status:  not indexed (run 'gaal index backfill' to index)
```

# `gaal resolve`

Purpose: resolve a short session ID to session metadata and derived artifact paths.

## Usage

```bash
gaal resolve [OPTIONS] [ID]
```

## Flags

| Flag | Description |
| --- | --- |
| `-H`, `--human` | Human-readable output (otherwise JSON) |
| `--engine <claude|codex>` | Filter by engine to disambiguate |

## JSON Output

- `session_id` — full session identifier from the index
- `short_id` — first 8 characters of `session_id`
- `engine` — `claude` or `codex`
- `jsonl_path` — absolute path to the source JSONL file
- `transcript_path` — expected rendered transcript markdown path
- `transcript_exists` — whether the transcript file exists on disk
- `handoff_path` — expected handoff markdown path
- `handoff_exists` — whether the handoff file exists on disk
- `session_type` — `standalone`, `coordinator`, or `subagent`
- `model` — model name (for example `claude-opus-4-6`)

## Real Examples

JSON output:

```bash
$ target/release/gaal resolve dc5e98dc
{
  "session_id": "dc5e98dc",
  "short_id": "dc5e98dc",
  "engine": "claude",
  "jsonl_path": "/Users/otonashi/.claude/projects/-Users-otonashi-thinking-pratchett-os-coordinator/dc5e98dc-5ed4-4de3-a440-d92defaeb9b1.jsonl",
  "transcript_path": "/Users/otonashi/.gaal/data/claude/sessions/2026/03/30/dc5e98dc.md",
  "transcript_exists": true,
  "handoff_path": "/Users/otonashi/.gaal/data/claude/handoffs/2026/03/30/dc5e98dc.md",
  "handoff_exists": false,
  "session_type": "coordinator",
  "model": "claude-opus-4-6"
}
```

Human-readable output (`-H`):

```bash
$ target/release/gaal resolve dc5e98dc -H
Session:    dc5e98dc (claude-opus-4-6, coordinator)
JSONL:      ~/.claude/projects/-Users-otonashi-thinking-pratchett-os-coordinator/dc5e98dc-5ed4-4de3-a440-d92defaeb9b1.jsonl
Transcript: ~/.gaal/data/claude/sessions/2026/03/30/dc5e98dc.md [ok]
Handoff:    ~/.gaal/data/claude/handoffs/2026/03/30/dc5e98dc.md [not generated]
```

## Exit Codes

- `0` — found
- `2` — ambiguous
- `3` — not found

## Related Commands

- [`gaal inspect`](./drill-down.md)
- [`gaal find-salt`](./self-id.md)

## Self-Handoff Protocol

1. Run `gaal salt` and capture the emitted token.
2. Echo that token into the live session so it is flushed into the session JSONL.
3. Run `gaal find-salt <token>` — this now returns full session context including JSONL path, model, session type, token counts, transcript path, and handoff status.
4. If a handoff is needed: `gaal create-handoff --jsonl <jsonl_path>`.

These must be separate tool calls because `salt` output has to be written into the session log before `find-salt` scans for it. If `find-salt` runs before the tool result is flushed, discovery can miss the active session.

## Related Commands

- [`gaal create-handoff`](./handoff.md)
- [`gaal transcript`](./drill-down.md)

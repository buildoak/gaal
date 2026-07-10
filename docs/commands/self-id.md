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

Purpose: scan Claude Code, Codex, Antigravity brain, and Grok visible session sources and return the first source containing a salt token in tool/action output. Returns enriched session context when the session is indexed, so agents can self-identify in a single call without chaining `inspect`/`transcript`/`recall`.

## Usage

```bash
gaal find-salt [OPTIONS] [SALT]
```

## Flags

- `-H`, `--human`
- `--engine <claude|codex|agy|grok>` — restrict the salt scan to one
  source engine

## JSON Output

When the session is indexed (has been processed by `gaal index backfill`):

- `session_id` — native source session identifier; agy uses the 8-character
  brain UUID prefix
- `engine` — `claude`, `codex`, `agy`, or `grok`
- `jsonl_path` — absolute path to the source file; for Grok this is the
  session directory because native Grok sessions are multi-file artifacts
- `indexed` — `true`
- `model` — model name (e.g. `claude-opus-4-6`)
- `cwd` — working directory of the session
- `session_type` — `standalone`, `coordinator`, or `subagent`
- `last_event_at` — timestamp of most recent event
- `turns` — total conversation turns
- `total_tokens` — combined input + output tokens when known
- `input_tokens` — input tokens when known
- `output_tokens` — output tokens when known
- `transcript_path` — expected path to rendered transcript markdown
- `transcript_exists` — whether the transcript file exists on disk
- `handoff.exists` — whether a handoff has been generated
- `handoff.generated_at` — handoff generation timestamp (if exists)

When not indexed:

- `session_id`, `engine`, `jsonl_path` — same as above
- `indexed` — `false`

Notes:

- The returned `session_id` is derived from the native source identity. Agy uses the first 8 characters of the Antigravity brain UUID. Grok keeps the full UUID.
- This command scans `~/.claude/projects/`, `~/.codex/`, Antigravity brain `transcript_full.jsonl` / `transcript.jsonl` files, and `${GROK_HOME:-~/.grok}/sessions/.../{updates.jsonl,chat_history.jsonl}`.
- Agy and Grok matching ignore user prompt echoes. The salt must appear in an executed action/tool output record such as command output, file/search output, image-generation output, or an error message.
- Enrichment is best-effort: if the DB is unavailable or the session is not indexed, the command still succeeds with the base 3 fields plus `"indexed": false`.

## Real Examples

Enriched output (indexed session):

```bash
$ gaal find-salt GAAL_SALT_40e4d9ceb25e0dd1
{"cwd":"/home/alex/src/agent-project","engine":"claude","handoff":{"exists":true,"generated_at":"2026-03-27T09:45:04Z"},"indexed":true,"input_tokens":883,"jsonl_path":"/home/alex/.claude/projects/.../5e54db27-a30e-455c-af24-26a3c55e511e.jsonl","last_event_at":"2026-03-27T09:45:20Z","model":"claude-opus-4-6","output_tokens":1400,"session_id":"5e54db27-a30e-455c-af24-26a3c55e511e","session_type":"coordinator","total_tokens":2283,"transcript_exists":true,"transcript_path":"/home/alex/.gaal/data/claude/sessions/2026/03/27/5e54db27.md","turns":31}
```

Human-readable output (`-H`):

```bash
$ gaal find-salt GAAL_SALT_40e4d9ceb25e0dd1 -H
Session: 5e54db27-a30e-455c-af24-26a3c55e511e
Engine:  claude (claude-opus-4-6)
Type:    coordinator
CWD:     /home/alex/src/agent-project
Tokens:  2K (883 in / 1K out) | 31 turns
Last:    2026-03-27T09:45:20.375Z
JSONL:   /home/alex/.claude/projects/.../5e54db27-a30e-455c-af24-26a3c55e511e.jsonl
Transcript: /home/alex/.gaal/data/claude/sessions/2026/03/27/5e54db27.md
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

Purpose: resolve a session ID, unique prefix, registered Hermes alias, or Grok last-8 alias to session metadata and derived artifact paths.

## Usage

```bash
gaal resolve [OPTIONS] [ID]
```

## Flags

| Flag | Description |
| --- | --- |
| `-H`, `--human` | Human-readable output (otherwise JSON) |
| `--engine <claude|codex|gemini|agy|hermes>` | Filter by engine to disambiguate |

## JSON Output

- `session_id` — canonical session identifier from the index. Hermes keeps the full native session ID here.
- `short_id` — artifact/display identifier. For Hermes this is the persisted 8-character alias.
- `engine` — `claude`, `codex`, `gemini`, `agy`, or `hermes`
- `jsonl_path` — legacy field name for the resolved source artifact path
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
  "jsonl_path": "/home/alex/.claude/projects/example-project/dc5e98dc-5ed4-4de3-a440-d92defaeb9b1.jsonl",
  "transcript_path": "/home/alex/.gaal/data/claude/sessions/2026/03/30/dc5e98dc.md",
  "transcript_exists": true,
  "handoff_path": "/home/alex/.gaal/data/claude/handoffs/2026/03/30/dc5e98dc.md",
  "handoff_exists": false,
  "session_type": "coordinator",
  "model": "claude-opus-4-6"
}
```

Human-readable output (`-H`):

```bash
$ target/release/gaal resolve dc5e98dc -H
Session:    dc5e98dc (claude-opus-4-6, coordinator)
JSONL:      ~/.claude/projects/example-project/dc5e98dc-5ed4-4de3-a440-d92defaeb9b1.jsonl
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
3. Run `gaal find-salt <token>` — this returns full indexed session context when available, including JSONL path, model, session type, token counts when known, transcript path, and handoff status.
4. If a handoff is needed: `gaal create-handoff --jsonl <jsonl_path> --dry-run`; generate only after review and a continuity need.

These must be separate tool calls because `salt` output has to be written into the session log before `find-salt` scans for it. If `find-salt` runs before the tool result is flushed, discovery can miss the active session.

## Related Commands

- [`gaal create-handoff`](./handoff.md)
- [`gaal transcript`](./drill-down.md)

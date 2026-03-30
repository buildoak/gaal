# gaal

Session observability CLI for Claude Code and Codex: index raw JSONL once, query sessions as structured artifacts.

## What It Does

- Parses Claude Code and Codex session logs, normalizes two different event models, and indexes them into SQLite plus Tantivy FTS.
- Answers the operational questions that matter: what happened, which session touched a file, what text appeared, which prior session should I resume, and where the session artifacts live.
- Indexes coordinator and subagent sessions for both engines, emits JSON by default, and switches to human-readable tables with `-H`.

## Install

```bash
git clone https://github.com/buildoak/gaal
cd gaal
cargo install --path .
```

For local development:

```bash
cargo build --release
```

Index existing sessions after install:

```bash
gaal index backfill
```

`create-handoff` uses `agent-mux` by default for extraction. Have `agent-mux` available before using that command, or configure an alternate provider.

## Quick Examples

Examples below are live output from this machine.

### Fleet View

```bash
$ target/release/gaal ls --limit 3 -H
ID        Task                Engine  Started      Duration  Tokens    Peak  Tools  Model    CWD 
--------  ------------------  ------  -----------  --------  --------  ----  -----  -------  ----
6a4b269f  # Lifter You ar...  codex   today 21:58  16s       9K / 737  13K   6      gpt-5.4  gaal
81174bae  # Lifter You ar...  codex   today 21:56  1m 17s    16K / 3K  22K   10     gpt-5.4  gaal
8d4cc563  # Lifter You ar...  codex   today 21:54  2m 12s    22K / 6K  29K   11     gpt-5.4  gaal
(filtered: hiding sessions with 0 tool calls and <30s duration. Use --all to show everything)
Showing 3 of 11355 sessions — use --limit N for more
```

### Inspect The Latest Session

```bash
$ target/release/gaal inspect latest --tokens -H
ID: 6a4b269f
Engine: codex
Model: gpt-5.4
Started: 2026-03-30T17:58:39.567Z
Duration: 16s
CWD: /Users/otonashi/thinking/building/gaal
Peak Context: 13K peak (max single-turn input incl. cache)
Files: read=0 written=0 edited=0
Ops: commands=6 errors=0 git=0
Tokens: in(non-cache)=9737 out=737  Peak(max turn incl. cache): 13K peak  Turns: 1  Tools: 6
Token breakdown: turns=1 avg_in/turn=9737 avg_out/turn=737 cost=$0.04
  Input total: 9737 (non-cached input tokens summed across the whole session)
  Peak context: 13K peak (max single-turn input = non-cached input + cache read + cache creation)
  Cache: read=16K creation=0
  Reasoning: 205
```

### Resolve A Short ID To Artifact Paths

```bash
$ target/release/gaal resolve dc5e98dc -H
Session:    dc5e98dc (claude-opus-4-6, coordinator)
JSONL:      ~/.claude/projects/-Users-otonashi-thinking-pratchett-os-coordinator/dc5e98dc-5ed4-4de3-a440-d92defaeb9b1.jsonl
Transcript: ~/.gaal/data/claude/sessions/2026/03/30/dc5e98dc.md [ok]
Handoff:    ~/.gaal/data/claude/handoffs/2026/03/30/dc5e98dc.md [not generated]
```

### Resolve In JSON

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

## Command Table

| Command | Purpose |
| --- | --- |
| `ls` | Fleet view across indexed sessions with engine, task, token, tool, type, and cwd filters. |
| `inspect` | Session detail with files, commands, token breakdown, session type, and subagent summaries. |
| `transcript` | Resolve or render the session transcript markdown. |
| `who` | Inverted attribution: which session read, wrote, ran, changed, or deleted a target. |
| `search` | BM25 full-text search over indexed facts via Tantivy. |
| `recall` | Ranked retrieval over generated handoffs for continuity and resumption. |
| `create-handoff` | Generate handoff markdown via LLM extraction; default provider is `agent-mux`. |
| `salt` | Emit a unique content token for self-identification. |
| `find-salt` | Locate the JSONL containing a salt token and return enriched session context when indexed. |
| `resolve` | Resolve a short session ID to source JSONL, transcript, handoff path, engine, and session type. |
| `tag` | Add, remove, or list session tags. |
| `index` | Backfill, reindex, prune, import, recover, and inspect index state. |

## Architecture

Disk discovery plus dual Claude/Codex parsers feed SQLite for structured queries and Tantivy for FTS; transcript rendering, path resolution, and self-identification still consult raw JSONL when file truth matters.

## Dual-Engine Subagent Indexing

Claude and Codex expose subagents differently, and gaal indexes both.

- Claude coordinators contribute subagent metadata from parent `toolUseResult` blocks, and child detail comes from `subagents/agent-*.jsonl`.
- Codex child sessions carry canonical linkage in `session_meta.forked_from_id`, with parent rollout records adding spawn and wait metadata.
- The result is one query surface for `ls`, `inspect`, `who`, `search`, `recall`, and `transcript`, even when work crossed coordinator and child sessions.

## Self-Handoff Protocol

Use the salt flow when an agent needs to identify its own live session and produce continuity material from that exact JSONL.

1. Emit a unique token:

   ```bash
   gaal salt
   ```

2. Locate the session JSONL containing that token:

   ```bash
   gaal find-salt GAAL_SALT_<16 hex chars>
   ```

3. Generate the handoff from the discovered path:

   ```bash
   gaal create-handoff --jsonl /absolute/path/from/find-salt
   ```

`salt` and `find-salt` must run as separate tool calls so the emitted token is present in the session log before discovery scans the file tree. When the session is already indexed, `find-salt` returns model, cwd, session type, token counts, transcript path, and handoff status in one call.

## AX Error Design

Human-mode errors are structured for recovery: state the failure, provide a working example, and point at the next move. Exit codes are stable: `0` success, `1` no results, `2` ambiguous ID, `3` not found, `10` no index, `11` parse error.

```bash
$ target/release/gaal who wrote src/commands/resolve.rs --since 1d -H
What went wrong: No sessions matched that attribution query.
Example: gaal who ran cargo --since 30d -H
Hint: Broaden the time range, remove extra filters, or try another verb such as `read`, `wrote`, or `touched`.
```

## Data Layout

- Source session logs: `~/.claude/projects/` and `~/.codex/`
- Gaal home: `~/.gaal/`
- Structured store: SQLite
- Full-text index: Tantivy
- Rendered artifacts: session transcripts and handoffs under `~/.gaal/data/`

## Docs

- [Docs landing page](./docs/README.md)
- [Getting started](./docs/getting-started.md)
- [Architecture](./docs/architecture.md)
- [Command reference](./docs/commands/)
- [Agent guide](./docs/agent-guide.md)
- [Self-identification and resolve](./docs/commands/self-id.md)
- [Formats and exit codes](./docs/formats.md)

## License

[MIT](./LICENSE)

Built by [Nick Oak](https://github.com/buildoak)

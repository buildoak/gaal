# gaal Documentation

`gaal` is a session observability CLI for Claude Code and Codex. It indexes raw JSONL session logs into SQLite plus Tantivy, then exposes fast agent-facing queries over sessions, facts, transcripts, tags, and handoffs.

This file is the canonical user document for the current CLI. If a root markdown file disagrees with this file, trust this file and the shipped CLI help text.

## Quick Start

### Build

```bash
cargo build --release
```

The installed binary is expected to be `target/release/gaal`. Per project convention, always verify changes against the release build before treating them as shipped.

### Install

```bash
cargo install --path .
```

### First index

```bash
gaal index backfill
```

If you want transcript markdown files written during indexing:

```bash
gaal index backfill --with-markdown
```

### Four commands to learn first

```bash
gaal ls -H
gaal inspect latest --tokens -H
gaal who wrote CLAUDE.md
gaal transcript latest
```

## Table of Contents

1. [Build and Installation](#build-and-installation)
2. [Output Conventions](#output-conventions)
3. [Architecture](#architecture)
4. [Data Model](#data-model)
5. [Session Lifecycle](#session-lifecycle)
6. [Commands](#commands)
7. [Operational Notes](#operational-notes)

## Build and Installation

### Requirements

- Rust toolchain
- Local access to session JSONL trees under `~/.claude/projects/` and/or `~/.codex/`
- Writable gaal home at `~/.gaal/`

### Build Modes

- `cargo build --release`
  - Required for real verification and the symlinked installed binary.
- `cargo run -- <command>`
  - Useful for local development and checking help text.

### gaal Home

`gaal` stores derived state under `~/.gaal/`:

```text
~/.gaal/
  index.db
  tantivy/
  data/
    claude/
      sessions/YYYY/MM/DD/<id>.md
      handoffs/YYYY/MM/DD/<id>.md
    codex/
      sessions/YYYY/MM/DD/<id>.md
      handoffs/YYYY/MM/DD/<id>.md
```

## Output Conventions

### Default mode

Most commands emit JSON by default. The main exceptions are:

- [`salt`](#salt) prints a raw token string.
- [`recall`](#recall) prints help and exits successfully when no query is given.
- [`transcript`](#transcript) prints markdown when `--stdout` is used.

### Human mode

Most commands accept global `-H` / `--human` and switch to table or card output.

### Error style

Errors are AX-style:

- Specific problem statement
- Copyable example
- Hint for the next action

Meaningful exit codes:

- `0` success
- `1` no results
- `2` ambiguous ID
- `3` not found
- `10` missing index
- `11` parse error

## Architecture

### High-level design

`gaal` is intentionally not a process monitor. It reads artifacts, indexes them, and answers questions about completed or in-progress sessions based on what is already on disk.

The main components are:

- Parser: dual Claude/Codex JSONL parsing
- SQLite: canonical structured store for sessions, facts, handoffs, and tags
- Tantivy: full-text search over indexed facts for [`search`](#search)
- Markdown renderer: transcript generation for [`transcript`](#transcript)
- Salt-based discovery: self-identification for [`salt`](#salt) and [`find-salt`](#find-salt)

### Subagent architecture: the two-source model

Subagent support is intentionally split across two sources:

1. Database-backed session metadata and indexed facts
2. Filesystem discovery of parent JSONL plus `subagents/agent-*.jsonl`

Why both exist:

- The DB is the fast query surface for `ls`, `inspect`, `who`, `recall`, tags, and most session metadata.
- The filesystem is still needed to discover subagent JSONLs, render transcripts, scan for salts, and recover detail that is only present in raw logs.

For Claude coordinator sessions specifically:

- Parent JSONL `toolUseResult` blocks provide aggregate subagent metadata such as `agentId`, duration, total tokens, status, prompt/description.
- Subagent JSONL files provide the full turn-by-turn trace and tool activity.

This is why:

- [`inspect`](#inspect) can show coordinator subagent summaries from indexed data.
- [`transcript`](#transcript) can re-render subagent-aware markdown from JSONL plus DB enrichment.
- [`who`](#who) and [`search`](#search) can attribute work to subagents while still surfacing the parent linkage.

## Data Model

### Sessions

The `sessions` table is the primary row per indexed session. Core fields:

- `id`
- `engine`
- `model`
- `cwd`
- `started_at`, `ended_at`, `last_event_at`
- `parent_id`
- `session_type` = `standalone`, `coordinator`, or `subagent`
- `jsonl_path`
- token totals
- `total_tools`, `total_turns`
- `peak_context`
- `last_indexed_offset`

### Facts

The `facts` table stores normalized session activity. Fact types:

- `file_read`
- `file_write`
- `command`
- `error`
- `git_op`
- `user_prompt`
- `assistant_reply`
- `task_spawn`

This table drives [`inspect`](#inspect), [`who`](#who), and [`search`](#search).

### Handoffs

The `handoffs` table stores extracted summaries used by [`create-handoff`](#create-handoff) and [`recall`](#recall):

- `headline`
- `projects`
- `keywords`
- `substance`
- `duration_minutes`
- `generated_at`
- `generated_by`
- `content_path`

### Tags

Session tags are stored in `session_tags` and managed through [`tag`](#tag).

### FTS Index

Tantivy indexes facts with session context fields such as:

- `session_id`
- `engine`
- `turn`
- `fact_type`
- `subject`
- `detail`
- `ts`
- `session_headline`

[`search`](#search) queries Tantivy; [`index backfill`](#index) and related index mutations rebuild it.

## Session Lifecycle

The common operator workflow is:

1. `gaal index backfill`
   - Discover JSONL files and populate SQLite plus Tantivy.
2. `gaal inspect latest`
   - Get the card view for one session.
3. `gaal who wrote <file>` or `gaal search <topic>`
   - Branch by attribution vs. semantic lookup.
4. `gaal transcript <id>`
   - Get the full markdown trace when the card is not enough.
5. `gaal create-handoff <id>`
   - Produce an LLM-generated continuity artifact.
6. `gaal recall <topic>`
   - Re-enter old work through handoff-ranked retrieval.

A self-handoff flow from inside a running agent session is:

```bash
SALT=$(gaal salt)
echo "$SALT"
JSONL=$(gaal find-salt "$SALT" | jq -r .jsonl_path)
gaal create-handoff --jsonl "$JSONL"
```

`salt` and `find-salt` must be separate calls so the tool-result is flushed to JSONL before scanning.

## Commands

## `inspect`

Purpose: session detail view for one or more sessions. This is the replacement for the older `show` command.

Usage:

```bash
gaal inspect [OPTIONS] [ID]
```

Flags:

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

JSON output:

- Default output is a compact session card with counts, token summary, tags, and `session_type`.
- Focused flags swap in specific payloads such as `files`, `commands`, `errors`, `git_ops`, or `trace`.
- Batch mode returns an array.

Human output:

- Card view with engine, model, duration, cwd, files, ops, and token notes.
- Coordinators include a subagent table.

Real example:

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

Related commands:

- [`transcript`](#transcript) for the full markdown trace
- [`who`](#who) for inverted file or command attribution

## `ls`

Purpose: fleet view over indexed sessions.

Usage:

```bash
gaal ls [OPTIONS]
```

Flags:

- `--engine <claude|codex>`
- `--since <duration|date>`
- `--before <date|timestamp>`
- `--cwd <substring>`
- `--tag <tag>`: repeatable, AND semantics
- `--sort <started|ended|tokens|cost|duration>`
- `--limit <n>`: default `10`
- `--aggregate`: return totals instead of session rows
- `--all`: include short/noise sessions
- `--skip-subagents`: hide subagent rows
- `-H`, `--human`

JSON output:

- Envelope with `query_window`, optional `filter`, `shown`, `total`, optional `total_unfiltered`, and `sessions`
- Aggregate mode returns totals and engine buckets

Human output:

- Tabular list with task/headline, engine, start time, duration, token totals, peak context, tools, model, cwd

Real example:

```bash
$ gaal ls --limit 2
{
  "query_window": {
    "from": "2026-01-08",
    "to": "2026-03-29T10:47:09Z"
  },
  "filter": "hiding sessions with 0 tool calls and <30s duration",
  "shown": 2,
  "total": 2,
  "total_unfiltered": 7202,
  "sessions": [
    {
      "id": "ab3f2e83",
      "engine": "codex",
      "model": "gpt-5.4",
      "cwd": "gaal",
      "started_at": "2026-03-29T10:46:13.988Z",
      "ended_at": "2026-03-29T10:46:56.904Z",
      "duration_secs": 42,
      "tokens": { "input": 52262, "output": 1653 },
      "peak_context": 48353,
      "tools_used": 14,
      "headline": "# Lifter Deep You are a disciplined builder for hard prob...",
      "session_type": "standalone"
    }
  ]
}
```

Human example:

```bash
$ gaal ls --limit 2 -H
ID        Task               Engine  Started      Duration  Tokens     Peak  Tools  Model    CWD
--------  -----------------  ------  -----------  --------  ---------  ----  -----  -------  ----
ab3f2e83  # Lifter Deep ...  codex   today 14:46  42s       52K / 1K   48K   14     gpt-5.4  gaal
```

Related commands:

- [`inspect`](#inspect) to drill into one row
- [`tag`](#tag) to filter or organize sessions

## `who`

Purpose: inverted attribution query. Ask which sessions read, wrote, ran, touched, changed, or deleted something.

Usage:

```bash
gaal who [OPTIONS] [VERB] [TARGET]
```

Verbs:

- `read`
- `wrote`
- `ran`
- `touched`
- `changed`
- `deleted`

Flags:

- `--since <duration|date>`: default `7d`
- `--before <date|timestamp>`
- `--cwd <substring>`
- `--engine <claude|codex>`
- `--tag <tag>`
- `--failed`: for `ran`, only non-zero command exits
- `--limit <n>`: default `10`
- `-F`, `--full`: return per-fact rows instead of grouped sessions
- `-H`, `--human`

JSON output:

- Default mode groups matches by session and returns `session_id`, `engine`, `latest_ts`, `fact_count`, `subjects`, `headline`, `session_type`, and optional `parent_id`
- `--full` returns one row per matched fact with `fact_type`, `subject`, `detail`, and timestamp

Human output:

- Brief session table by default
- For subagent rows, parent-to-subagent attribution is shown inline

Real example:

```bash
$ gaal who wrote CLAUDE.md --limit 2
{
  "query_window": {
    "from": "2026-03-22",
    "to": "2026-03-29"
  },
  "shown": 2,
  "total": 4,
  "sessions": [
    {
      "session_id": "a2608f02",
      "engine": "claude",
      "latest_ts": "2026-03-28T18:25:28.470Z",
      "fact_count": 2,
      "subjects": ["CLAUDE.md"],
      "headline": null,
      "session_type": "subagent",
      "parent_id": "7d5d03e4"
    }
  ]
}
```

Related commands:

- [`search`](#search) for content search instead of fact attribution
- [`inspect`](#inspect) to examine a returned session

## `recall`

Purpose: ranked continuity retrieval over generated handoffs.

Usage:

```bash
gaal recall [OPTIONS] [QUERY]
```

Flags:

- `--days-back <n>`: default `14`
- `--limit <n>`: default `3`
- `--format <summary|handoff|brief|full|eywa>`: default `brief`
- `--substance <n>`: default `1`
- `-H`, `--human`

Output formats:

- `brief`: compact session summary blocks
- `summary`: structured fields only
- `handoff`: structured summary plus raw handoff content
- `full`: summary plus handoff, files, and errors
- `eywa`: legacy markdown-oriented layout

If no query is passed, `recall` prints usage help and exits successfully.

Real example:

```bash
$ gaal recall subagent --limit 2 -H
━━━ Session 2b0db33c (2026-03-29) ━━━
Headline: Refined gaal’s subagent architecture, shipped the first working subagent engine, and closed the main AX gaps around who, ls, inspect, and transcript rendering.
Projects: gaal, coordinator
Keywords: gaal, subagent, transcript, who, BACKLOG.md
Substance: 3 | Duration: 327m | Score: 44.9
```

Related commands:

- [`create-handoff`](#create-handoff) to generate the artifacts recall uses
- [`search`](#search) when you need raw fact-level matches instead of ranked handoffs

## `search`

Purpose: full-text search over indexed facts using Tantivy.

Usage:

```bash
gaal search [OPTIONS] [QUERY]
```

Flags:

- `--since <duration|date>`: default `30d`
- `--cwd <substring>`
- `--engine <claude|codex>`
- `--field <prompts|replies|commands|errors|files|all>`: default `all`
- `--context <n>`: default `2`
- `--limit <n>`: default `20`
- `-H`, `--human`

JSON output:

- Envelope with `query_window`, `shown`, `total`, and `results`
- Each result includes `session_id`, `engine`, `turn`, `fact_type`, `subject`, `snippet`, `ts`, `score`, `session_headline`, `session_type`, and optional `parent_id`

Human output:

- Ranked table optimized for quick scanning

Real example:

```bash
$ gaal search subagent --limit 2
{
  "query_window": {
    "from": "2026-02-27",
    "to": "2026-03-29"
  },
  "shown": 2,
  "total": 13,
  "results": [
    {
      "session_id": "aea2ddc4",
      "engine": "claude",
      "turn": 29,
      "fact_type": "command",
      "subject": "grep -rn \"20\\b\\|subagent.*limit\\|table.*cap\\|MAX.*SUBAGENT\\|SUBAGENT.*MAX\\|top.*subagent\\|subagent.*",
      "snippet": "grep -rn \"20\\b\\|subagent.*limit\\|table.*cap\\|MAX.*SUBAGENT\\|SUBAGENT.*MAX\\|top.*subagent\\|subagent.*top\" /Users/otonashi/thinking/building/gaal/src/ --include=\"*.rs\" | grep -v \"target\" | head -20",
      "ts": "2026-03-29T05:29:59.567Z",
      "score": 15.346081,
      "session_headline": "",
      "session_type": "subagent",
      "parent_id": "2b0db33c"
    }
  ]
}
```

Related commands:

- [`who`](#who) for precise file/command attribution
- [`recall`](#recall) for handoff-ranked continuity lookup

## `create-handoff`

Purpose: generate handoff markdown via LLM extraction, either for one session or in batch.

Usage:

```bash
gaal create-handoff [OPTIONS] [ID]
```

Flags:

- `--jsonl <path>`: explicit JSONL override
- `--engine <claude|codex>`: extraction engine override
- `--model <model>`
- `--prompt <path>`
- `--provider <agent-mux|openrouter>`: default `agent-mux`
- `--format <string>`: default `eywa-compatible`
- `--batch`
- `--since <duration|date>`: default `7d`
- `--parallel <n>`: default `1`
- `--min-turns <n>`: default `3`
- `--this`: prefer the current detected session rather than a parent
- `--dry-run`: preview candidates only
- `-H`, `--human`

JSON output:

- Single-session mode returns an array of handoff results with `session_id`, `handoff_path`, `headline`, `projects`, `keywords`, and `substance`
- Batch mode returns per-session status rows
- `--dry-run` still returns JSON rows, with candidate summary lines printed to stderr

Real example:

```bash
$ gaal create-handoff --batch --dry-run --since 1d --min-turns 3
[
  {
    "session_id": "aed14881",
    "status": "pending",
    "handoff_path": null,
    "error": null,
    "duration_secs": 0.0
  },
  {
    "session_id": "a7e8c6f6",
    "status": "pending",
    "handoff_path": null,
    "error": null,
    "duration_secs": 0.0
  }
]
```

Related commands:

- [`recall`](#recall) consumes generated handoffs
- [`transcript`](#transcript) is the raw source document to inspect when extraction looks suspicious
- [`salt`](#salt) and [`find-salt`](#find-salt) support self-handoff flows

## `transcript`

Purpose: return or render the markdown transcript for a session.

Usage:

```bash
gaal transcript [OPTIONS] [ID]
```

Flags:

- `--force`: re-render even if cached markdown already exists
- `--stdout`: print markdown instead of JSON path metadata
- `-H`, `--human`

Behavior:

- Default mode resolves or renders the transcript file and returns path metadata.
- `--stdout` prints the markdown body.
- If no ID is provided, the command prints help and exits successfully.

JSON output:

- `path`
- `size_bytes`
- `estimated_tokens`
- `warning`

Human output:

- Three-line summary with path, size, estimated tokens, and warning

Real example:

```bash
$ gaal transcript latest
{
  "path": "/Users/otonashi/.gaal/data/codex/sessions/2026/03/29/ab3f2e83.md",
  "size_bytes": 16034,
  "estimated_tokens": 4008,
  "warning": "~4K tokens. Recommend reading via subagent, not coordinator context."
}
```

Related commands:

- [`inspect`](#inspect) for the compact session card
- [`create-handoff`](#create-handoff) for summarized continuity artifacts

## `salt`

Purpose: generate a unique salt token for content-addressed self-identification.

Usage:

```bash
gaal salt
```

Flags:

- `-H`, `--human`

Output:

- Raw token string on stdout, not JSON
- Format: `GAAL_SALT_<16 hex chars>`

Real example:

```bash
$ gaal salt
GAAL_SALT_d0a6e1d5530bf6c9
```

Related commands:

- [`find-salt`](#find-salt) resolves the JSONL containing the token
- [`create-handoff`](#create-handoff) can then run on that JSONL

## `find-salt`

Purpose: scan Claude and Codex JSONL trees and return the first file containing a salt token.

Usage:

```bash
gaal find-salt [OPTIONS] [SALT]
```

Flags:

- `-H`, `--human`

JSON output:

- `session_id`
- `engine`
- `jsonl_path`

Notes:

- The returned `session_id` is derived from the JSONL filename stem, so Codex and Claude shapes differ.
- This command scans `~/.claude/projects/` and `~/.codex/`.

Real example:

```bash
$ gaal find-salt GAAL_SALT_d0a6e1d5530bf6c9
{"engine":"codex","jsonl_path":"/Users/otonashi/.codex/sessions/2026/03/29/rollout-2026-03-29T14-46-13-019d3933-90c8-7cc3-b974-a910ab3f2e83.jsonl","session_id":"rollout-2026-03-29T14-46-13-019d3933-90c8-7cc3-b974-a910ab3f2e83"}
```

Related commands:

- [`salt`](#salt)
- [`create-handoff`](#create-handoff)

## `index`

Purpose: index maintenance and corpus mutation commands.

Usage:

```bash
gaal index <SUBCOMMAND> [OPTIONS]
```

Subcommands:

### `index backfill`

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

### `index status`

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

### `index reindex`

Usage:

```bash
gaal index reindex <ID>
```

Flags:

- `-H`, `--human`

Output:

- JSON summary with `session_id` and `facts`

### `index import-eywa`

Usage:

```bash
gaal index import-eywa [PATH]
```

Flags:

- `-H`, `--human`

Output:

- JSON summary with `imported`, `skipped`, and `errors`

### `index prune`

Usage:

```bash
gaal index prune --before <DATE>
```

Flags:

- `--before <date>`
- `-H`, `--human`

Output:

- JSON object with `before` and `deleted`

Operational note: any command that mutates facts rebuilds the Tantivy index afterwards.

## `tag`

Purpose: add, remove, or list session tags.

Usage:

```bash
gaal tag [OPTIONS] [ID] [TAGS]...
```

Flags:

- `--remove`: remove tags instead of adding them
- `-H`, `--human`

Modes:

- `gaal tag ls`: list all distinct tags
- `gaal tag <id> <tag1> <tag2>`: add tags
- `gaal tag <id> <tag1> --remove`: remove tags

JSON output:

- `tag ls` returns a JSON array of strings
- add/remove returns `{ "session_id": "...", "action": "added|removed", "tags": [...] }`

Real example:

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

Related commands:

- [`ls`](#ls) supports `--tag`
- [`inspect`](#inspect) includes a session’s `tags`

## Operational Notes

### Time filters

Most query commands accept flexible time expressions:

- relative: `1h`, `7d`, `2w`, `today`
- absolute dates: `2026-03-29`
- timestamps: RFC3339 or `YYYY-MM-DDTHH:MM` where supported

### Noise filtering

By default, [`ls`](#ls) hides sessions with zero tool calls and duration under 30 seconds. Use `--all` to include them.

### Search index rebuilds

These commands rebuild Tantivy:

- `gaal index backfill`
- `gaal index reindex`
- `gaal index prune`
- `gaal index import-eywa`

### When to use which command

- Need the latest high-level view: [`ls`](#ls)
- Need one session card: [`inspect`](#inspect)
- Need full markdown trace: [`transcript`](#transcript)
- Need file/command attribution: [`who`](#who)
- Need raw content matches: [`search`](#search)
- Need ranked continuity: [`recall`](#recall)
- Need continuity artifact generation: [`create-handoff`](#create-handoff)
- Need self-discovery from inside a session: [`salt`](#salt) + [`find-salt`](#find-salt)
- Need corpus maintenance: [`index`](#index)
- Need labeling: [`tag`](#tag)

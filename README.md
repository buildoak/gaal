# gaal

Session observability for AI coding agents. Parses Claude Code, Codex, and Gemini CLI session logs, indexes into SQLite + Tantivy FTS, answers any question about any session in seconds.

[![crates.io](https://img.shields.io/crates/v/gaal.svg)](https://crates.io/crates/gaal)
[![CI](https://github.com/buildoak/gaal/actions/workflows/ci.yml/badge.svg)](https://github.com/buildoak/gaal/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS-lightgrey)

11,000+ sessions. 372K facts. 895 handoffs indexed. Three-engine. 12 commands.

---

## What It Does

Claude Code, Codex, and Gemini CLI emit session logs -- 10-50MB blobs of undocumented, engine-specific event streams. Raw, they're useless. Gaal parses all three formats, normalizes three different event models, and turns raw traces into queryable artifacts.

- **Fleet view** across thousands of sessions, all engines, one table. Filter by engine, model, session type, time window, CWD, or token count.
- **Drill into any session** -- files touched, commands run, subagent swarms, token breakdown, peak context, cost estimate.
- **Attribution** -- which session wrote that file? Which agent ran that command? Traces through coordinator-subagent chains with arrow notation.
- **Continuity** -- recall past work via BM25 full-text search and ranked handoff retrieval. Each new session is less amnesic than the last.
- **Self-identification** -- agents find their own session mid-flight via the salt protocol. Emit a token, scan for it, get back the full session context.

---

## Install

Requires Rust 1.80+.

```bash
git clone https://github.com/buildoak/gaal.git
cd gaal
cargo install --path .
```

Index existing sessions:

```bash
gaal index backfill
gaal index status
```

`create-handoff` dispatches to an LLM via [agent-mux](https://github.com/buildoak/agent-mux). Have agent-mux installed before using that command.

---

## Examples

All output below is from live runs on a real machine. These are not mockups.

### Fleet Overview

```bash
gaal ls -H --limit 5
```

```
ID        Type   Task             Engine  Started      Duration  Tokens    Peak  Tools  Model            CWD
--------  -----  ---------------  ------  -----------  --------  --------  ----  -----  ---------------  -----------
a6eec194  [sub]  **Task: Desi...  claude  today 22:56  2m 15s    13 / 179  61K   28     claude-opus-4-6  coordinator
abcf7430  [sub]  **Fix: Subag...  claude  today 22:43  3m 33s    36 / 1K   64K   44     claude-opus-4-6  coordinator
07282aef  -      # Scout You ...  codex   today 22:42  13s       12K / 1K  15K   2      gpt-5.4-mini     radiation
a97eff05  [sub]  You are a se...  claude  today 22:37  4m 12s    33 / 2K   71K   42     claude-opus-4-6  coordinator
a4b8c9c5  [sub]  **Investigat...  claude  today 22:35  2m 23s    20 / 1K   67K   28     claude-opus-4-6  coordinator
(filtered: hiding sessions with 0 tool calls and <30s duration. Use --all to show everything)
Showing 5 of 11366 sessions
```

All engines in one table. `[sub]` marks subagent sessions. Tokens column shows `input / output`. Peak is max single-turn context window usage. The default filter hides trivial sessions -- `--all` shows everything.

Narrow the view:

```bash
gaal ls --session-type coordinator --since 1d -H    # parent sessions today
gaal ls --subagent-type gsd-heavy --since 3d         # GSD dispatches this week
gaal ls --engine codex --limit 10 -H                 # Codex sessions only
gaal ls --engine gemini --limit 10 -H                # Gemini sessions only
gaal ls --model claude-opus-4-6 --since 7d -H        # Opus sessions this week
```

### Coordinator Drill-Down

```bash
gaal inspect 23af2de9 -H
```

```
ID: 23af2de9
Engine: claude
Model: claude-opus-4-6
Started: 2026-03-30T09:45:43.771Z
Duration: 33374s
CWD: /Users/otonashi/thinking/pratchett-os/coordinator
Peak Context: 164K peak (max single-turn input incl. cache)
Subagents (28):
ID        Model              Tokens  Duration
--------  -----------------  ------  --------
ac526a97  claude-sonnet-4-6  62K     26s
a46e7c77  claude-sonnet-4-6  69K     30s
a81e1444  claude-sonnet-4-6  135K    53s
a7aaffc9  claude-sonnet-4-6  998K    3m 9s
a7f4bc0f  claude-opus-4-6    1.3M    3m 4s
a68852f9  claude-opus-4-6    40.6M   32m 40s
afc3009c  claude-sonnet-4-6  5.0M    22m 21s
a43292dd  claude-opus-4-6    8.3M    29m 48s
...
Files: read=10 written=1 edited=2
Ops: commands=28 errors=2 git=0
Tokens: in(non-cache)=2087 out=26663  Peak(max turn incl. cache): 164K peak  Turns: 141  Tools: 98
```

A coordinator session with 28 subagents. The subagent table shows per-agent model, total tokens consumed, and wall-clock duration. One subagent burned 40.6M tokens over 32 minutes -- that's a GSD-Heavy swarm doing deep work.

Add `--tokens` for the full breakdown including cache read/creation, per-turn averages, and cost estimate.

### Session Resolve

```bash
gaal resolve 23af2de9 -H
```

```
Session:    23af2de9 (claude-opus-4-6, coordinator)
JSONL:      ~/.claude/projects/-Users-otonashi-thinking-pratchett-os-coordinator/23af2de9-....jsonl
Transcript: ~/.gaal/data/claude/sessions/2026/03/30/23af2de9.md [ok]
Handoff:    ~/.gaal/data/claude/handoffs/2026/03/30/23af2de9.md [not generated]
```

Maps a short ID to all associated artifacts. `[ok]` means the transcript has been rendered. `[not generated]` means no handoff exists yet -- run `gaal create-handoff 23af2de9` to generate one.

JSON output drops the `-H` flag:

```bash
gaal resolve 23af2de9
```

```json
{
  "session_id": "23af2de9",
  "engine": "claude",
  "jsonl_path": "~/.claude/projects/.../23af2de9-....jsonl",
  "transcript_path": "~/.gaal/data/claude/sessions/2026/03/30/23af2de9.md",
  "transcript_exists": true,
  "handoff_path": "~/.gaal/data/claude/handoffs/2026/03/30/23af2de9.md",
  "handoff_exists": false,
  "session_type": "coordinator",
  "model": "claude-opus-4-6"
}
```

### Attribution

Which sessions wrote a file:

```bash
gaal who wrote CLAUDE.md --limit 3 -H
```

```
Searching last 7 days (2026-03-23 -> 2026-03-30)
Session              Engine  When         Facts  Subjects   Headline
-------------------  ------  -----------  -----  ---------  --------
23af2de9 -> aee2c84b  claude  today 22:35  1      CLAUDE.md  -
23af2de9             claude  today 21:33  1      CLAUDE.md  -
0e49b03c -> aa755b14  claude  today 09:14  1      CLAUDE.md  -
Showing 3 of 7 results
```

The arrow notation (`23af2de9 -> aee2c84b`) means the write happened inside a subagent spawned by that coordinator. Attribution flows through the chain -- gaal traces subagent activity back to the parent.

Six verbs: `read`, `wrote`, `ran`, `touched`, `changed`, `deleted`. Each maps to the corresponding tool operations in the JSONL.

```bash
gaal who ran cargo --since 30d -H              # who ran cargo commands
gaal who read src/main.rs --since 7d -H        # who read this file
gaal who touched "*.toml" --since 14d -H       # who interacted with any TOML file
```

### Full-Text Search

```bash
gaal search "handoff" --limit 3 -H
```

```
Score  Session   Engine  Turn  Type     Time          Snippet
-----  --------  ------  ----  -------  ------------  -----------------------------------------------
13.96  a3fa9934  claude  59    command  Mar 09 11:23  grep -n "gaal handoff --batch\|handoff.*--ba...
13.96  acompact  claude  59    command  Mar 09 11:23  grep -n "gaal handoff --batch\|handoff.*--ba...
13.93  a3fa9934  claude  47    command  Mar 09 11:22  grep -n "gaal handoff\|agent-mux.*handoff\|...
```

BM25 ranking over indexed facts via Tantivy. Searches across commands, file operations, errors, and extracted content from all sessions. Results show the session, the turn where the match occurred, and a snippet.

### Recall

Retrieve past context by topic:

```bash
gaal recall "continuity" --limit 2 -H
```

```
--- Session a244dee3 (2026-03-29) ---
Headline: Read and summarized the handoff file for coordinator continuity.
Projects: coordinator
Keywords: handoff, summary, coordinator, gaal, session-analysis
Substance: 1 | Duration: 0m | Score: 30.5

--- Session 0a795b39 (2026-02-07) ---
Headline: Multi-agent context mechanics experiment bootstrapped
Projects: pratchett-os, eywa
Keywords: multi-agent-orchestration, codex-pipeline, context-continuity
Substance: 2 | Duration: 260m | Score: 0.5
```

Ranked retrieval over generated handoffs. Handoffs are LLM-extracted summaries of what a session accomplished -- the headline, projects involved, keywords, and a substance score. `recall` searches them with BM25 and returns the most relevant prior context for your current task.

For a specific session's handoff:

```bash
gaal recall --id 23af2de9 -H
```

### Self-Identification (Salt Protocol)

Agents need to know their own session ID mid-flight. The salt protocol solves this in two steps.

**Step 1:** Emit a unique token. This gets written into the session's JSONL as a tool result.

```bash
gaal salt
# GAAL_SALT_716a02ca9642c721
```

**Step 2:** In a separate tool call (JSONL must flush between steps), scan for the token:

```bash
gaal find-salt GAAL_SALT_716a02ca9642c721 -H
```

```
Session: 23af2de9-2eac-49e2-bc6a-f7841568b818
Engine:  claude (claude-opus-4-6)
Type:    coordinator
Tokens:  18K (961 in / 17K out) | 102 turns
JSONL:   ~/.claude/projects/.../23af2de9-....jsonl
Handoff: no
```

Now the agent has its own session ID, JSONL path, and can generate its own handoff via `create-handoff`. The two-step split is mandatory -- the salt must exist in the JSONL file before `find-salt` scans the file tree.

### Handoff Generation

```bash
gaal create-handoff 23af2de9
```

Dispatches to an LLM (via agent-mux) that reads the session transcript and produces a structured handoff: headline, projects, keywords, substance score, and a summary of what happened. Generated handoffs are stored at `~/.gaal/data/{engine}/handoffs/` and indexed for `recall`.

This command costs money -- it runs an LLM extraction pass. Use it on sessions that matter for continuity, not on every throwaway Codex dispatch.

---

## Command Reference

### Query

| Command | What it does |
|---------|-------------|
| `gaal ls` | Fleet view. Filters: `--engine`, `--model`, `--session-type`, `--subagent-type`, `--since`, `--cwd`, `--limit`, `--sort`, `--all`, `--skip-subagents` |
| `gaal inspect <id>` | Session detail. Files, commands, subagents, token breakdown. `--tokens` for full accounting, `--trace` for turn-by-turn |
| `gaal who <verb> <target>` | Attribution. Verbs: `read`, `wrote`, `ran`, `touched`, `changed`, `deleted`. `--since`, `--limit` |
| `gaal search <query>` | BM25 full-text search over all indexed facts. `--limit`, `--since` |
| `gaal recall [topic]` | Ranked handoff retrieval. `--id` for a specific session, `--format brief\|full`, `--limit` |
| `gaal resolve <id>` | Short ID to JSONL path, transcript path, handoff path, engine, type, model |
| `gaal transcript <id>` | Session transcript as rendered markdown. `--stdout` dumps inline, otherwise returns the file path |

### Generate

| Command | What it does |
|---------|-------------|
| `gaal create-handoff <id>` | LLM-powered handoff extraction. Costs $. Requires agent-mux |

### Identity

| Command | What it does |
|---------|-------------|
| `gaal salt` | Emit a unique content token for self-identification |
| `gaal find-salt <token>` | Locate the JSONL containing that token, return enriched session metadata |

### Maintain

| Command | What it does |
|---------|-------------|
| `gaal index backfill` | Scan disk for new sessions and index them |
| `gaal index reindex` | Re-parse all indexed sessions from source JSONL |
| `gaal index prune` | Remove entries for sessions whose JSONL no longer exists on disk |
| `gaal index status` | Index statistics: session counts, fact counts, engine breakdown |
| `gaal index recover-orphans` | Find and index subagent JSONL files not linked to any parent |
| `gaal tag add <id> <tag>` | Tag a session |
| `gaal tag remove <id> <tag>` | Remove a tag |
| `gaal tag ls <id>` | List tags on a session |

### Full CLI

```
Agent session observability CLI

Usage: gaal [OPTIONS] <COMMAND>

Commands:
  ls              Fleet view across sessions
  inspect         Session details with optional focused views
  transcript      Get session transcript markdown
  who             Inverted query: which session did X to Y
  search          Full-text search over indexed facts
  recall          Semantic session retrieval
  resolve         Resolve a short session ID to paths and metadata
  salt            Generate a random salt token for session identification
  find-salt       Find the first JSONL file containing the provided salt token
  create-handoff  Generate/create a session handoff markdown via LLM extraction
  index           Index maintenance and backfill operations
  tag             Apply or remove tags on a session

Options:
  -H, --human    Human-readable output (otherwise JSON)
  -h, --help     Print help
  -V, --version  Print version
```

---

## Architecture

```
Session files on disk
  -> discovery/ (scan ~/.claude/projects/, ~/.codex/, and ~/.gemini/tmp/)
  -> parser/ (Claude/Codex/Gemini parsers -> events -> facts)
  -> db/ (SQLite for structured data + Tantivy for FTS)
  -> commands/ (query DB and Tantivy)
  -> output/ (JSON or human-readable tables)
```

**Three-engine.** Claude Code, Codex, and Gemini CLI each use different session formats (JSONL or JSON). Gaal has a dedicated parser for each, normalizes all three into the same fact model, and exposes one query surface. You never need to know which engine produced a session -- `ls`, `inspect`, `who`, `search`, and `recall` work the same across all engines.

**Session taxonomy.** Three types, deterministically classified from JSONL structure:

| Type | Meaning |
|------|---------|
| `standalone` | Normal session, no subagents |
| `coordinator` | Parent session that spawned subagents via Agent tool |
| `subagent` | Child session spawned by a coordinator |

Coordinator-subagent relationships are indexed from two sources: parent `toolUseResult` blocks (fleet-level metadata) and child JSONL files (full turn-by-turn trace). Attribution via `who` traces through both.

**Output modes.** JSON by default -- stable, composable with `jq`, assertable with `jq -e`. `-H` switches to human-readable tables with adaptive column widths. Agents use JSON. Humans use `-H`.

---

## What gaal Does NOT Do

Deliberately excluded. These were built, tested, and removed because they caused more problems than they solved.

| Excluded | Why |
|----------|-----|
| Process monitoring (`gaal active`) | Too fragile. PID-based session tracking broke across reparenting, SSH, tmux, and container boundaries |
| Live tailing (`--live`, `--watch`) | Gaal is a query tool, not a daemon. Tailing JSONL is better done with `tail -f` |
| Stuck/loop detection | Heuristic garbage. Wrong more often than right. 50+ edge cases for near-zero value |
| Velocity / context % | Calculated metrics were unreliable and misleading. Removed rather than shipped lies |
| PID-based parent-child linking | 1 out of 2,433 sessions ever linked via PID. Replaced by the salt protocol |

If a monitoring feature seems missing, it was probably here once and got killed for cause.

---

## Error Design

Errors are structured for agent self-recovery. Every error message includes what went wrong, a working example, and a hint for what to try next.

```bash
gaal who badverb test
```

```json
{
  "error": "Unrecognized verb `badverb`. Valid verbs: read, wrote, ran, touched, changed, deleted.",
  "example": "gaal who ran cargo --since 7d -H",
  "hint": "Pick one of the listed verbs, then optionally provide a target to narrow the match."
}
```

Exit codes are stable contracts:

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | No results matched |
| `2` | Ambiguous session ID (multiple matches) |
| `3` | Session not found |
| `10` | No index (run `gaal index backfill`) |
| `11` | Parse error (bad input) |

An agent encountering any error can parse the JSON, extract the example, and self-correct. No cryptic failures.

---

## Scale

```bash
gaal index status
```

```json
{
  "sessions_total": 11366,
  "sessions_by_engine": { "claude": 7861, "codex": 3505 },
  "facts_total": 372400,
  "handoffs_total": 895,
  "db_size_bytes": 431050752,
  "oldest_session": "2026-01-08",
  "newest_session": "2026-03-30T18:56:24.949Z"
}
```

411 MB SQLite database. 82 days of sessions from two engines. Full backfill from cold takes under 2 minutes. Incremental indexing runs in seconds. Queries return in single-digit milliseconds.

---

## Built With

- [Rust](https://www.rust-lang.org/) (edition 2021, stable toolchain)
- [rusqlite](https://github.com/rusqlite/rusqlite) (bundled SQLite)
- [tantivy](https://github.com/quickwit-oss/tantivy) (BM25 full-text search)
- [clap](https://github.com/clap-rs/clap) (CLI parsing, derive mode)
- [serde](https://serde.rs/) + [serde_json](https://github.com/serde-rs/json)

No async runtime. Synchronous by design.

## Docs

Full documentation lives in [`docs/`](./docs/):

- [Getting started](./docs/getting-started.md)
- [Architecture](./docs/architecture.md)
- [Command reference](./docs/commands/)
- [Agent guide](./docs/agent-guide.md)
- [Formats and exit codes](./docs/formats.md)

## License

MIT -- [Nick Oak](https://nickoak.com), 2026

---

Built by [Nick Oak](https://nickoak.com)

# Verb Reference — gaal

Complete flag and output schema reference for all 12 commands.

---

## Table of Contents

1. [ls](#1-ls) — Fleet view
2. [show](#2-show) — Full session record
3. [inspect](#3-inspect) — Operational snapshot
4. [who](#4-who) — Inverted queries
5. [search](#5-search) — Full-text search
6. [recall](#6-recall) — Semantic session retrieval
7. [create-handoff](#7-create-handoff) — LLM-powered handoff generation
8. [index](#8-index) — Index management
9. [active](#9-active) — Live process discovery
10. [tag](#10-tag) — Session tagging
11. [salt](#11-salt) — Salt token generation
12. [find-salt](#12-find-salt) — JSONL discovery by salt

---

## 1. ls

Fleet view. The entry point. Lists sessions from the SQLite index.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--status <STATUS>` | string (repeatable) | all | Filter: active, idle, stuck, completed, failed |
| `--engine <ENGINE>` | string | all | Filter: claude, codex |
| `--since <DURATION>` | string | none | Lower bound: `1h`, `3d`, `2w`, `2026-03-01`, `today` |
| `--before <DATE>` | string | none | Upper bound: `2026-03-03T17:00`, `today`, `yesterday` |
| `--cwd <PATH>` | string | none | Substring match on working directory |
| `--tag <TAG>` | string (repeatable) | none | Filter by tag (AND logic when multiple) |
| `--sort <FIELD>` | string | started | Options: started, ended, tokens, duration, status |
| `--limit <N>` | int | 10 | Max results. Output shows "showing N of M" footer |
| `--aggregate` | flag | off | Return totals instead of session list |
| `-F, --full` | flag | off | Verbose output (all fields) |
| `-H` | flag | off | Human-readable table |

### Output Schema (JSON array)

```json
[{
  "id": "string",
  "engine": "string",           // "claude" | "codex"
  "model": "string",            // e.g. "claude-opus-4-6"
  "status": "string",           // active|idle|stuck|completed|failed|unknown
  "cwd": "string",
  "started_at": "string",       // RFC3339
  "ended_at": "string|null",    // RFC3339 or null if active
  "duration_secs": "number",
  "tokens": {
    "input": "number",
    "output": "number"
  },
  "tools_used": "number",
  "headline": "string|null"     // null if no handoff generated
}]
```

### `--aggregate` Output Schema

```json
{
  "sessions": "number",
  "total_input_tokens": "number",
  "total_output_tokens": "number",
  "estimated_cost_usd": "number",
  "by_engine": { "claude": "number", "codex": "number" },
  "by_status": { "completed": "number", "active": "number", ... }
}
```

### Example

```bash
gaal ls --engine claude --since 3d --limit 5 -H
```

---

## 2. show

Full session record. Progressive disclosure workhorse.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | positional | required | Session ID prefix, full ID, or `latest` |
| `--files [read\|write\|all]` | string | all (when bare) | File operations filter |
| `--errors` | flag | off | Errors and non-zero exits only |
| `--commands` | flag | off | Bash/exec commands only |
| `--git` | flag | off | Git operations only |
| `--tokens` | flag | off | Token usage breakdown |
| `--trace` | flag | off | Full event timeline (progressive disclosure level 2) |
| `--source` | flag | off | Raw JSONL dump path (level 3) |
| `--ids <ID,ID,...>` | string | none | Batch mode: multiple sessions |
| `--tag <TAG>` | string | none | Batch mode: all sessions with tag |
| `-H` | flag | off | Human-readable |

**Fact filter behavior:** When any of `--files`, `--errors`, `--commands`, `--git` is specified, only that section appears in output. Other sections are suppressed.

### Output Schema (JSON object)

```json
{
  "id": "string",
  "engine": "string",
  "model": "string",
  "status": "string",
  "cwd": "string",
  "started_at": "string",
  "ended_at": "string|null",
  "duration_secs": "number",
  "tokens": { "input": "number", "output": "number" },
  "turns": "number",
  "tools_used": "number",
  "headline": "string|null",
  "tags": ["string"],
  "files": {
    "read": ["string"],         // file paths
    "written": ["string"],
    "edited": ["string"]
  },
  "commands": [{
    "cmd": "string",
    "exit_code": "number",
    "ts": "string"
  }],
  "errors": [{
    "tool": "string",
    "cmd": "string",
    "exit_code": "number",
    "snippet": "string",
    "ts": "string"
  }],
  "git_ops": [{
    "op": "string",             // commit, push, checkout, etc.
    "message": "string",
    "ts": "string"
  }]
}
```

**`show -H` renders a summary card** (headline, duration, tokens, key stats) rather than the full JSON dump.

### Examples

```bash
# Files modified by latest session
gaal show latest --files write

# Batch: show multiple sessions
gaal show --ids a1b2c3d4,e5f6g7h8

# Full event timeline
gaal show f15a045c --trace
```

---

## 3. inspect

Operational snapshot. What is a session doing RIGHT NOW? Degrades gracefully for completed sessions.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | positional | none | Session ID prefix, full ID, or `latest` |
| `--watch` | flag | off | Re-poll every 2s (like top) |
| `--active` | flag | off | All running sessions in one call |
| `--ids <ID,ID,...>` | string | none | Batch mode |
| `-H` | flag | off | Human-readable |

### Output Schema (JSON object)

```json
{
  "id": "string",
  "status": "string",
  "pid": "number|null",        // null for completed sessions
  "engine": "string",
  "model": "string",
  "uptime_secs": "number",
  "process": {                  // null for completed sessions
    "cpu_pct": "number",
    "rss_mb": "number",
    "threads": "number"
  },
  "tokens": {
    "total": "number",
    "ctx_window": "number",
    "ctx_limit": "number"
  },
  "current_turn": {             // null/absent for completed sessions
    "number": "number",
    "started_at": "string",
    "elapsed_secs": "number",
    "last_action": {
      "kind": "string",
      "summary": "string"
    },
    "actions_this_turn": "number"
  },
  "velocity": {
    "actions_per_minute_5m": "number",
    "tokens_per_minute_5m": "number"
  },
  "recent_errors": [{
    "tool": "string",
    "cmd": "string",
    "exit_code": "number",
    "age_secs": "number"
  }]
}
```

### Examples

```bash
# Inspect latest active session
gaal inspect latest

# Health dashboard for all running sessions
gaal inspect --active -H

# Watch mode (refreshes every 2s)
gaal inspect f15a045c --watch
```

---

## 4. who

Inverted queries. "Which session did X to Y?"

### Verbs

| Verb | Matches | fact_type filter |
|------|---------|-----------------|
| `read` | File read operations | `file_read` |
| `wrote` | File write/edit operations | `file_write` |
| `ran` | Bash/exec commands | `command` |
| `touched` | Any interaction (broadest) | `file_read` OR `file_write` OR `command` |
| `installed` | Package installs (verb expansion) | `command` + patterns |
| `changed` | Files written + git commits | `file_write` OR `git_op` |
| `deleted` | rm/unlink commands or empty-writes | `command` OR `file_write` |

**Folder support:** Target ending with `/` matches recursively (`subject LIKE '{target}%'`).

**Invalid verb:** Exit code 11 (parse error).

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<verb>` | positional | required | read, wrote, ran, touched, installed, changed, deleted |
| `<target>` | positional | required | File path, command fragment, or search term |
| `--since <DURATION>` | string | 7d | Time window |
| `--before <DATE>` | string | none | Upper bound |
| `--cwd <PATH>` | string | none | Restrict to sessions in directory |
| `--engine <ENGINE>` | string | all | Filter by engine |
| `--tag <TAG>` | string | none | Filter by tag |
| `--failed` | flag | off | Only failed commands (for `ran` verb) |
| `--limit <N>` | int | 10 | Max results. Output shows "showing N of M" indicator |
| `-F, --full` | flag | off | Show full per-fact output including detail fields |
| `-H` | flag | off | Human-readable |

**v0.1.0 improvements:** Verb matching uses command names (not substring). Search window is displayed in output. Scope disclaimers are included when results may be incomplete.

### Output Schema (JSON array)

```json
[{
  "session_id": "string",
  "engine": "string",
  "ts": "string",               // RFC3339
  "fact_type": "string",        // file_read, file_write, command, git_op
  "subject": "string|null",     // file path for file ops, null for commands
  "detail": "string",           // full command text or detail
  "session_headline": "string|null"
}]
```

### Examples

```bash
# IMPORTANT: capture output to variable first (pipe gotcha)
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.[0].session_id'

# Who ran a specific command
OUTPUT=$(gaal who ran "cargo test" --since 30d --limit 5)
echo "$OUTPUT" | jq '.'

# Folder-recursive: who modified anything under coordinator/
OUTPUT=$(gaal who wrote coordinator/ --since 14d)
echo "$OUTPUT" | jq '.'
```

---

## 5. search

Full-text search across session content. BM25 via Tantivy. Fact-level granularity.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<query>` | positional | required | Search terms |
| `--since <DURATION>` | string | 30d | Time window |
| `--cwd <PATH>` | string | none | Restrict to sessions in directory |
| `--engine <ENGINE>` | string | all | Filter by engine |
| `--field <FIELD>` | string | all | Restrict to: prompts, replies, commands, errors, files, all |
| `--context <N>` | int | 2 | Lines of context around match |
| `--limit <N>` | int | 20 | Max results |
| `-H` | flag | off | Human-readable |

### Output Schema (JSON array)

```json
[{
  "session_id": "string",
  "engine": "string",
  "turn": "number",
  "fact_type": "string",
  "subject": "string|null",
  "snippet": "string",          // matched text with context
  "ts": "string",
  "score": "number",            // BM25 relevance score
  "session_headline": "string|null"
}]
```

Results sorted by `score` descending.

### Example

```bash
gaal search "gaussian moat" --field commands --limit 5 -H
```

---

## 6. recall

Semantic session retrieval. "What do I know about X?" IDF + recency scoring over handoff metadata.

**Prerequisite:** Requires handoffs in the index. Check `gaal index status | jq '.handoffs_total'`. Returns exit 1 when handoffs table is empty.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `[query]` | positional | none | Topic query (optional — defaults to recent substantive sessions) |
| `--days-back <N>` | int | 14 | Recency window |
| `--limit <N>` | int | 3 | Max sessions |
| `--format <FMT>` | string | summary | Options: summary, handoff, brief, full |
| `--substance <N>` | int | 1 | Minimum substance score |
| `-H` | flag | off | Human-readable |

### Format Options

| Format | Use case |
|--------|----------|
| `summary` | Default. Session metadata + score. |
| `brief` | System-prompt-sized (3-5 lines per session). Cold start injection. |
| `handoff` | Full handoff markdown content. |
| `full` | Summary + handoff + files + errors. Maximum detail. |

### Output Schema (JSON array)

```json
[{
  "session_id": "string",
  "date": "string",             // YYYY-MM-DD
  "headline": "string",
  "projects": ["string"],
  "keywords": ["string"],
  "substance": "number",        // 0-3
  "duration_minutes": "number",
  "score": "number",            // relevance score
  "handoff_path": "string"      // path to handoff MD
}]
```

### Example

```bash
gaal recall "peekaboo" --format brief --limit 5
```

---

## 7. create-handoff

LLM-powered handoff generation. Dispatches to agent-mux. **Costs money.**

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `[id\|today]` | positional | optional | Session ID or `today`. Auto-detects if omitted |
| `--jsonl <JSONL>` | string | none | Explicit JSONL file path (skip PID detection, used by salt protocol) |
| `--engine <ENGINE>` | string | from config | agent-mux engine for LLM |
| `--model <MODEL>` | string | from config | Model for extraction |
| `--prompt <PATH>` | string | `~/.gaal/prompts/handoff.md` | Custom extraction prompt |
| `--provider <PROVIDER>` | string | agent-mux | LLM provider: agent-mux, openrouter |
| `--format <FMT>` | string | eywa-compatible | Output format |
| `--batch` | flag | off | Run batch mode for multiple sessions |
| `--since <DURATION>` | string | 7d | Time window for batch mode |
| `--parallel <N>` | int | 1 | Max concurrent batch workers |
| `--min-turns <N>` | int | 3 | Minimum turns for batch candidates |
| `--this` | flag | off | Extract nearest detected session (not parent) |
| `--dry-run` | flag | off | Preview batch candidates without processing |

### Output Schema (JSON array)

```json
[{
  "session_id": "string",
  "handoff_path": "string",     // path to generated handoff MD
  "headline": "string",
  "projects": ["string"],
  "keywords": ["string"],
  "substance": "number"
}]
```

### Pipeline

1. Gather session facts from SQLite
2. Load extraction prompt
3. Dispatch to LLM via agent-mux
4. Write handoff MD to `~/.gaal/data/{engine}/handoffs/YYYY/MM/DD/<id>.md`
5. Update `handoffs` table

### Example

```bash
# Generate handoff for a specific session
gaal create-handoff f15a045c

# Generate handoffs for all today's sessions
gaal create-handoff today
```

---

## 8. index

Build, manage, and inspect the index.

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `backfill` | Index all existing JSONL files |
| `status` | Index health report |
| `reindex <id>` | Force re-index one session |
| `import-eywa [path]` | Import legacy eywa handoff-index.json |
| `prune --before <date>` | Remove old facts (keep session entries) |

### backfill Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--engine <ENGINE>` | string | all | Only index specific engine |
| `--since <DATE>` | string | none | Only sessions after date |
| `--force` | flag | off | Re-index even if already indexed |

### status Output Schema

```json
{
  "db_path": "string",
  "db_size_bytes": "number",
  "sessions_total": "number",
  "sessions_by_engine": { "claude": "number", "codex": "number" },
  "sessions_by_status": { "completed": "number", "active": "number", ... },
  "facts_total": "number",
  "handoffs_total": "number",
  "last_indexed_at": "string",
  "oldest_session": "string",
  "newest_session": "string"
}
```

### Example

```bash
# Full backfill
gaal index backfill

# Check index health
gaal index status | jq '{sessions: .sessions_total, facts: .facts_total, handoffs: .handoffs_total}'

# Import eywa data
gaal index import-eywa
```

---

## 9. active

Live process discovery. What's running RIGHT NOW? Queries live PIDs, not the archive.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--engine <ENGINE>` | string | all | Filter: claude, codex |
| `--watch` | flag | off | Re-poll every 2s |
| `-H` | flag | off | Human-readable |

### Output Schema (JSON array)

```json
[{
  "id": "string",
  "engine": "string",
  "model": "string",
  "pid": "number",
  "cwd": "string",
  "uptime_secs": "number",
  "cpu_pct": "number",
  "rss_mb": "number",
  "status": "string",           // active|idle
  "last_action": "string",      // e.g. "Bash: cargo test"
  "last_action_age_secs": "number",
  "tmux_session": "string|null"
}]
```

### Example

```bash
gaal active -H
```

---

## 10. tag

Post-factum session tagging. Tags are strings, stored in `session_tags` join table.

### Usage

```bash
# Add one tag
gaal tag f15a045c "reddit-sweep"

# Add multiple tags
gaal tag f15a045c "reddit-sweep" "research"

# Remove a tag
gaal tag f15a045c --remove "research"
```

Tags are filterable via `--tag` on: `ls`, `show`, `inspect`, `who`.

---

## 11. salt

Generate a unique salt token for session self-identification. The token is printed to stdout and lands in the JSONL as part of the tool-result.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-H` | flag | off | Human-readable output |

### Output

```json
"gaal:salt:a7f3b2c1"
```

Plain string token. Use in the self-handoff protocol (step 1).

---

## 12. find-salt

Find the JSONL file containing a previously generated salt token. Scans recent JSONL files for the token string.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `<SALT>` | positional | required | Salt token to search for |
| `-H` | flag | off | Human-readable output |

### Output Schema (JSON object)

```json
{
  "jsonl_path": "string",
  "engine": "string",
  "session_id": "string"
}
```

Use `jsonl_path` with `gaal create-handoff --jsonl` for self-handoff.

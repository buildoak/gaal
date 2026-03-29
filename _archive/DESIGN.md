<!-- Archived 2026-03-29: superseded by DOCS.md / BACKLOG.md -->
---
date: 2026-03-03
status: design-v3
engine: claude-opus
origin: coordinator sessions 708b7c4b, current
---

# Gaal — Agent Session Observability CLI

Named after Gaal Dornick — the mathematician who arrived on Trantor and had to make sense of a civilization's worth of accumulated data. Fitting for a tool that reads the raw traces of agent work and makes them queryable.

CLI-first. Agents-first. JSON output by default. Like peekaboo — simple verbs, composable.

## The One Problem

No engine-agnostic, machine-readable, queryable representation of what agent sessions DID exists. Raw JSONL is too verbose and engine-specific. Session markdowns are good for humans and LLM summarization but not for programmatic queries. Everything downstream — fleet view, child visibility, cross-session awareness, cost tracking — is blocked by this absence.

## Problems Solved

18 validated problems from user experience + Reddit/Twitter community sweep (2026-03-03).

### Phase 1 (6 problems):
- **P1:** Eywa is manual + agent-mux contamination → `gaal recall`, `gaal ls --children`
- **P2:** Child agent work is invisible → `gaal show --tree`, `gaal show --children`
- **P3:** No fleet view → `gaal ls`, `gaal active`
- **P10:** Session health / hang detection → `gaal inspect`, `gaal ls --stuck`
- **P11:** Post-session diff / "what did it change?" → `gaal who wrote`, `gaal show --files write`
- **P18:** Session recovery / agent-caused data loss → `gaal who wrote`, `gaal show`

### Phase 2 (5 problems):
- **P4:** No cross-session awareness → `gaal search`, `gaal recall`
- **P7:** Cost blindness → `gaal ls --aggregate`
- **P8:** Attention / human-agent signaling → `gaal inspect --watch`, `gaal inspect --active`
- **P9:** Runaway loops / unbounded execution → `gaal inspect` velocity + stuck signals
- **P14:** Cold start / session-zero → `gaal recall --format brief`

### Out of scope (not observability problems):
P5 (silent compaction), P6 (agent deception), P12 (permission bypass), P13 (context self-consumption), P15 (no convergence signal), P16 (cross-tool compat — partially solved by engine-agnostic design), P17 (MCP pollution)

Full problem set with evidence documented separately (see Research Artifacts below).

---

## Data Directory

Gaal owns its data under `~/.gaal/`. Single root. Self-contained.

```
~/.gaal/
  index.db                        # SQLite (sessions + facts + handoffs + session_tags)
  config.toml                     # LLM defaults, agent-mux path, stuck thresholds
  prompts/
    handoff.md                    # Customizable handoff extraction prompt
  tantivy/                        # Full-text search index
  dispatch.jsonl                  # agent-mux dispatch log (parent-child links)
  data/
    claude/
      sessions/                   # Full session MDs (from JSONL parsing)
        YYYY/MM/DD/<id>.md
      handoffs/                   # LLM-generated handoff MDs
        YYYY/MM/DD/<id>.md
    codex/
      sessions/
        YYYY/MM/DD/<id>.md
      handoffs/
        YYYY/MM/DD/<id>.md
```

**Source of truth:** Raw JSONL files (written by Claude Code / Codex CLI).
**Gaal produces:** SQLite index + session MDs + handoff MDs. All derived from JSONL.
**Adding an engine:** One new directory under `data/`, one new parser.

Pipeline: `gaal index backfill` reads raw JSONL → writes session MDs + SQLite rows. `gaal handoff` generates handoff MDs via LLM.

---

## Operations — 9 Verbs + 1 Utility

### 1. `gaal ls`

Fleet view. The entry point.

```bash
gaal ls                              # all sessions, most recent first
gaal ls --status active              # filter by status
gaal ls --engine codex               # filter by engine
gaal ls --since 1d --before 2026-03-03T17:00 --cwd /path
gaal ls --stuck                      # sessions needing human attention
gaal ls --tag "reddit-sweep"         # filter by tag
gaal ls --sort cost|duration|started
gaal ls --limit 10
gaal ls --children                   # include child/worker sessions (excluded by default)
gaal ls --aggregate                  # token/cost totals instead of session list
gaal ls -H                           # human-readable table
```

Flags:
- `--status <STATUS>` — filter: active|idle|stuck|completed|failed (repeatable)
- `--engine <ENGINE>` — filter: claude|codex
- `--since <DURATION>` — lower bound: 1h, 3d, 2w, 2026-03-01
- `--before <DATE>` — upper bound: 2026-03-03T17:00
- `--cwd <PATH>` — substring match on working directory
- `--stuck` — shorthand: stuck + long idle sessions needing attention
- `--tag <TAG>` — filter by tag (repeatable, AND logic)
- `--sort <FIELD>` — started (default), ended, tokens, duration, status
- `--limit <N>` — max results (default 50)
- `--children` — include child/worker sessions (off by default — prevents agent-mux contamination)
- `--aggregate` — return totals (sessions count, total tokens, estimated cost) instead of session list
- `-H` — human-readable table

Output (JSON array):
```json
[{
  "id": "f15a045c",
  "engine": "claude",
  "model": "claude-opus-4-6",
  "status": "completed",
  "cwd": "/home/user/projects/myproject",
  "started_at": "2026-03-02T22:55:59Z",
  "ended_at": "2026-03-03T11:14:22Z",
  "duration_secs": 44303,
  "parent_id": null,
  "child_count": 6,
  "tokens": { "input": 450000, "output": 120000 },
  "tools_used": 847,
  "headline": "Harvested 33 inspiration seeds, built twitter-peekaboo skill"
}]
```

`--aggregate` output:
```json
{
  "sessions": 12,
  "total_input_tokens": 2400000,
  "total_output_tokens": 680000,
  "estimated_cost_usd": 47.20,
  "by_engine": { "claude": 10, "codex": 2 },
  "by_status": { "completed": 10, "active": 1, "failed": 1 }
}
```

**Solves:** P3, P7 (via --aggregate), P10

### 2. `gaal show <id>`

Full session record. The workhorse. Progressive disclosure.

```bash
gaal show f15a045c                   # full session record
gaal show f15a045c --files read      # files the session consumed
gaal show f15a045c --files write     # files the session produced/modified
gaal show f15a045c --files all       # both, separated into read/written arrays
gaal show f15a045c --errors          # errors and non-zero exits
gaal show f15a045c --commands        # bash/exec commands
gaal show f15a045c --git             # git operations
gaal show f15a045c --tokens          # token usage breakdown by turn
gaal show f15a045c --tree            # spawn hierarchy (children, their children)
gaal show f15a045c --children        # child session summaries inline
gaal show f15a045c --trace           # full event timeline (level 2)
gaal show f15a045c --source          # raw JSONL path (level 3)
gaal show latest                     # most recent session
gaal show --ids a1b2,c3d4,e5f6      # batch: multiple sessions at once
gaal show --tag "reddit-sweep"       # batch: all sessions with tag
gaal show f15a045c -H                # human-readable
```

Flags:
- `--files [read|write|all]` — file operations. `read` = Read/Grep/Glob calls. `write` = Write/Edit calls. Default when bare: all.
- `--errors` — errors and non-zero exit codes only
- `--commands` — bash/exec commands only
- `--git` — git operations only
- `--tokens` — token usage breakdown
- `--tree` — recursive spawn hierarchy
- `--children` — child session summaries inline
- `--trace` — full event timeline (progressive disclosure level 2)
- `--source` — raw JSONL dump path (progressive disclosure level 3)
- `--ids <ID,ID,...>` — batch mode: multiple sessions in one call
- `--tag <TAG>` — batch mode: all sessions with this tag
- `-H` — human-readable

Output (JSON):
```json
{
  "id": "54fd2b6c",
  "engine": "claude",
  "model": "claude-opus-4-6",
  "status": "completed",
  "cwd": "/path",
  "started_at": "...",
  "ended_at": "...",
  "duration_secs": 44280,
  "tokens": { "input": 450000, "output": 120000 },
  "turns": 47,
  "tools_used": 847,
  "parent_id": null,
  "children": ["a1b2c3d4", "e5f6g7h8"],
  "headline": "...",
  "files": {
    "read": ["/path/to/file1.rs", "/path/to/file2.md"],
    "written": ["/path/to/file3.rs"],
    "edited": ["/path/to/file4.toml"]
  },
  "commands": [
    { "cmd": "cargo build", "exit_code": 0, "ts": "..." }
  ],
  "errors": [
    { "tool": "Bash", "cmd": "cargo test", "exit_code": 1, "snippet": "...", "ts": "..." }
  ],
  "git_ops": [
    { "op": "commit", "message": "fix auth flow", "ts": "..." }
  ]
}
```

`--tree` output (JSON, recursive):
```json
{
  "id": "54fd2b6c",
  "intent": "boot Jenkins",
  "status": "completed",
  "duration_secs": 44280,
  "children": [
    { "id": "a1b2c3d4", "intent": "Reddit sweep", "status": "completed", "duration_secs": 1847, "children": [] },
    { "id": "e5f6g7h8", "intent": "explore codebase", "status": "completed", "duration_secs": 480, "children": [
      { "id": "f9g0h1i2", "intent": "read parsers", "status": "completed", "duration_secs": 60, "children": [] }
    ]}
  ]
}
```

**Solves:** P1, P2, P11, P18

### 3. `gaal inspect <id|latest>`

Operational snapshot. What is it doing RIGHT NOW?

```bash
gaal inspect f15a045c                # snapshot
gaal inspect f15a045c --watch        # re-poll every 2s (like top)
gaal inspect latest                  # most recent active session
gaal inspect --active                # health for ALL running sessions
gaal inspect --ids a1b2,c3d4        # batch: multiple sessions
gaal inspect f15a045c -H
```

Flags:
- `--watch` — re-poll every 2s, clear-and-reprint
- `--active` — all running sessions in one call
- `--ids <ID,ID,...>` — batch mode
- `-H` — human-readable

Output (JSON):
```json
{
  "id": "54fd2b6c",
  "status": "active",
  "pid": 12345,
  "engine": "claude",
  "model": "claude-opus-4-6",
  "uptime_secs": 3847,
  "process": {
    "cpu_pct": 12.3,
    "rss_mb": 245,
    "threads": 8
  },
  "context": {
    "tokens_used": 145000,
    "tokens_limit": 200000,
    "pct_used": 72.5
  },
  "current_turn": {
    "number": 23,
    "started_at": "2026-03-03T14:22:00Z",
    "elapsed_secs": 45,
    "last_action": { "kind": "Bash", "summary": "cargo test" },
    "actions_this_turn": 7
  },
  "velocity": {
    "actions_per_minute_5m": 4.2,
    "tokens_per_minute_5m": 3200
  },
  "stuck_signals": {
    "silence_secs": 0,
    "loop_detected": false,
    "context_pct": 72.5,
    "permission_blocked": false
  },
  "recent_errors": [
    { "tool": "Bash", "cmd": "npm test", "exit_code": 1, "age_secs": 120 }
  ]
}
```

**For completed sessions:** Same schema, `pid`/`process` are null, `current_turn` becomes `last_turn`. Degrades gracefully.

**Solves:** P10, P8, P9

### 4. `gaal who <verb> <target>`

Inverted queries. "Which session did X to Y?"

```bash
# Core verbs
gaal who read CLAUDE.md                          # who read this file?
gaal who wrote coordinator/MEMORY.md             # who modified this file?
gaal who ran "cargo test"                        # who ran this command?

# Folder support
gaal who wrote coordinator/                      # anything modified under this dir
gaal who read docs/research/                     # anything read under this dir

# Semantic verbs (command clustering)
gaal who touched peekaboo                        # files OR commands mentioning "peekaboo"
gaal who installed tantivy                       # matches pip/npm/brew/cargo/apt install patterns
gaal who changed "*.rs"                          # files written + git commits touching pattern
gaal who deleted tmp/                            # rm commands or empty-writes to target path

# Filters
gaal who wrote src/main.rs --since 7d
gaal who wrote src/ --since 2026-02-01 --before 2026-03-01
gaal who ran "git push" --cwd /path/to/project
gaal who ran --failed --since today              # all failed commands today
gaal who touched peekaboo --engine claude
gaal who installed tantivy --limit 5
gaal who wrote coordinator/ --tag "research"
gaal who wrote coordinator/ -H
```

Verbs and matching logic:

| Verb | Matches | Details |
|------|---------|---------|
| `read` | `file_read` facts | Read/Grep/Glob tool calls where target appears in subject |
| `wrote` | `file_write` facts | Write/Edit tool calls where target appears in subject |
| `ran` | `command` facts | Bash calls where target appears in command string |
| `touched` | `file_read` OR `file_write` OR `command` | Any interaction — broadest verb |
| `installed` | `command` facts + verb expansion | Expands to: install, add, brew, cargo add, pip install, npm install, apt install, go get |
| `changed` | `file_write` OR `git_op` facts | Files written + git commits touching pattern |
| `deleted` | `command` OR `file_write` facts | rm/unlink commands or empty-writes to target |

Semantic verb expansion (hardcoded, <30 lines):
```rust
fn expand_verb(verb: &str) -> Vec<&str> {
    match verb {
        "installed" => vec!["install", "add ", "brew ", "cargo add", "pip install", "npm install", "apt install", "go get"],
        "deleted"   => vec!["rm ", "rm -", "unlink", "remove", "del "],
        _           => vec![]
    }
}
```

Folder support: when target ends with `/`, match is `subject LIKE '{target}%'` — recursive.

Flags:
- `--since <DURATION>` — time window (default: 7d)
- `--before <DATE>` — upper bound
- `--cwd <PATH>` — restrict to sessions in this directory
- `--engine <ENGINE>` — filter by engine
- `--tag <TAG>` — filter by tag
- `--failed` — only commands with non-zero exit (for `ran` verb)
- `--limit <N>` — max results (default 10)
- `-H` — human-readable

Output (JSON array):
```json
[{
  "session_id": "54fd2b6c",
  "engine": "claude",
  "ts": "2026-03-02T14:00:00Z",
  "fact_type": "command",
  "subject": null,
  "detail": "pip install tantivy",
  "session_headline": "Built search index for gaal"
}]
```

**Solves:** P4, P11, P18

### 5. `gaal search <query>`

Full-text search across session content. BM25 via Tantivy.

```bash
gaal search "gaussian splatting"                         # free-text across all content
gaal search "reddit sweep" --field commands              # only in commands
gaal search "permission denied" --field errors           # only in errors
gaal search "MEMORY.md" --field files                    # only in file paths
gaal search "gaussian" --since 30d --engine claude
gaal search "auth flow" --cwd /path/to/project
gaal search "gaussian splatting" --context 3
gaal search "gaussian splatting" --limit 5 -H
```

Flags:
- `--since <DURATION>` — time window (default: 30d)
- `--cwd <PATH>` — restrict to sessions in this directory
- `--engine <ENGINE>` — filter by engine
- `--field <FIELD>` — restrict to: prompts|replies|commands|errors|files|all (default: all)
- `--context <N>` — lines of context around match (default: 2)
- `--limit <N>` — max results (default 20)
- `-H` — human-readable

Each indexable unit is a **fact** (not a session). Gives per-fact granularity.

Output (JSON array):
```json
[{
  "session_id": "54fd2b6c",
  "engine": "claude",
  "turn": 23,
  "fact_type": "command",
  "subject": "cargo test",
  "snippet": "...running cargo test -- 3 failures in auth module...",
  "ts": "2026-03-02T14:00:00Z",
  "score": 12.4,
  "session_headline": "Built auth module for gaal"
}]
```

**Solves:** P4, P14

### 6. `gaal recall [query]`

Eywa replacement. Semantic session retrieval. "What do I know about X?"

```bash
gaal recall                                      # most recent substantive sessions
gaal recall "gaussian moat"                      # sessions about this topic
gaal recall "reddit sweep myproject" --days-back 30
gaal recall "peekaboo" --limit 5
gaal recall "gaussian" --format handoff          # full handoff markdown
gaal recall "gaussian" --format brief            # system-prompt-sized (3-5 lines per session)
gaal recall "gaussian" --format full             # summary + handoff + files + errors
gaal recall "gaussian" -H
```

Flags:
- `--days-back <N>` — recency window (default: 14)
- `--limit <N>` — max sessions (default: 3)
- `--format <FMT>` — summary|handoff|brief|full (default: summary)
- `--substance <N>` — minimum substance score (default: 1)
- `-H` — human-readable

Scoring (ported from eywa):
1. Tokenize query, strip stopwords
2. Score sessions: project name match (3x), keyword match (2x), IDF-weighted
3. Recency decay: within window `1 + 1/sqrt(age_days)`, outside `0.5^((age - days_back)/7)`
4. Duration bonus: `1 + 0.1 * ln(duration_minutes + 1)`
5. Filter substance=0 sessions
6. No hits → fall back to most-recent substantive

Output (JSON array):
```json
[{
  "session_id": "54fd2b6c",
  "date": "2026-03-02",
  "headline": "Harvested 33 inspiration seeds, built twitter-peekaboo skill",
  "projects": ["myproject", "backend-api"],
  "keywords": ["twitter-peekaboo", "inspiration-harvesting"],
  "substance": 2,
  "duration_minutes": 738,
  "score": 18.7,
  "handoff_path": "~/.gaal/data/claude/handoffs/2026/03/02/54fd2b6c.md"
}]
```

**Why separate from `search`?** Different algorithms, different data. Search = BM25 over raw content (facts). Recall = IDF + recency over curated metadata (sessions). They compose: `recall` finds the session, `show --trace` drills in.

**Solves:** P1, P14

### 7. `gaal handoff <id|today>`

**Killer feature.** LLM-powered handoff generation with customizable engine/model/prompt.

```bash
gaal handoff f15a045c                                    # default: engine + prompt from config
gaal handoff f15a045c --engine codex --model spark-high  # specific engine/model
gaal handoff f15a045c --prompt ~/.gaal/prompts/custom.md # custom extraction prompt
gaal handoff f15a045c --provider openrouter              # direct API, skip agent-mux
gaal handoff today                                       # all today's sessions → handoffs
```

Flags:
- `--engine <ENGINE>` — agent-mux engine for LLM extraction (default from config)
- `--model <MODEL>` — model for extraction (default from config)
- `--prompt <PATH>` — custom extraction prompt (default: `~/.gaal/prompts/handoff.md`)
- `--provider <PROVIDER>` — LLM provider: agent-mux|openrouter (default: agent-mux)
- `--format <FMT>` — output format (default: eywa-compatible)

Pipeline:
1. Gather session facts from SQLite index
2. Load extraction prompt (customizable)
3. Dispatch to LLM via agent-mux (or direct API)
4. Write handoff MD to `~/.gaal/data/{engine}/handoffs/YYYY/MM/DD/<id>.md`
5. Update `handoffs` table with headline, projects, keywords, substance

Default handoff format (eywa-compatible):
```markdown
---
session_id: f15a045c
date: 2026-03-02
engine: claude
model: claude-opus-4-6
generated_by: codex/spark-high
---
## Headline
<one-line summary>
## What Happened
<structured summary of key actions>
## Key Decisions
<decisions made during session>
## Open Threads
<unfinished work, next steps>
## Key Files
<files created/modified with brief descriptions>
```

**Solves:** P1

### 8. `gaal index <subcommand>`

Build, manage, and inspect the index.

```bash
gaal index backfill                              # index all existing JSONL files
gaal index backfill --engine claude              # only Claude sessions
gaal index backfill --since 2026-02-01           # only sessions after date
gaal index backfill --force                      # re-index even if already indexed

gaal index status                                # index health report
gaal index reindex <id>                          # force re-index one session
gaal index import-eywa [path]                    # import legacy eywa handoff-index.json
gaal index prune --before <date>                 # remove old facts (keep session entries)
```

`index status` output:
```json
{
  "db_path": "~/.gaal/index.db",
  "db_size_bytes": 67000000,
  "sessions_total": 6700,
  "sessions_by_engine": { "claude": 5963, "codex": 737 },
  "sessions_by_status": { "completed": 6650, "active": 8, "failed": 42 },
  "facts_total": 335000,
  "handoffs_total": 609,
  "last_indexed_at": "2026-03-03T14:00:00Z",
  "oldest_session": "2026-01-08",
  "newest_session": "2026-03-03"
}
```

`import-eywa` logic:
1. Read `handoff-index.json` from `data/eywa/`
2. Insert each entry into `handoffs` table
3. Parse handoff markdown files for file/command references → `facts` table
4. Create stub session entries for sessions not yet backfilled
5. Copy handoff MDs to `~/.gaal/data/claude/handoffs/` structure

### 9. `gaal active`

Live process discovery. What's running RIGHT NOW?

```bash
gaal active                          # all running sessions
gaal active --engine claude          # just Claude Code
gaal active --watch                  # re-poll every 2s
gaal active -H
```

Output (JSON array):
```json
[{
  "id": "54fd2b6c",
  "engine": "claude",
  "model": "claude-opus-4-6",
  "pid": 12345,
  "cwd": "/path",
  "uptime_secs": 3847,
  "cpu_pct": 12.3,
  "rss_mb": 245,
  "context_pct": 72.5,
  "status": "active",
  "last_action": "Bash: cargo test",
  "last_action_age_secs": 15,
  "tmux_session": "claude-os-2",
  "stuck_signals": { "silence_secs": 0, "loop_detected": false, "permission_blocked": false }
}]
```

**Why separate from `ls`?** `ls` queries the archive (SQLite), may be stale. `active` queries ONLY live processes — catches sessions not yet indexed. For "what is happening RIGHT NOW" → `active`. For "recent sessions that are still running" → `ls --status active`.

**Solves:** P3, P8, P10

### Utility: `gaal tag`

Post-factum tagging. Tags are LLM-generated (via `gaal handoff` pipeline) or manually applied.

```bash
gaal tag f15a045c "reddit-sweep"                 # add tag
gaal tag f15a045c "reddit-sweep" "research"      # multiple tags
gaal tag f15a045c --remove "research"            # remove tag
```

`--tag` filter available on: `ls`, `show`, `inspect`, `who`.

Tags stored in `session_tags` join table.

---

## Agent-Mux Integration (Bidirectional)

### agent-mux → Gaal: Dispatch Log

agent-mux writes to `~/.gaal/dispatch.jsonl` on every worker spawn:
```json
{"ts": "2026-03-03T14:00:00Z", "caller_pid": 12345, "child_session_id": "a1b2c3d4", "engine": "codex", "model": "spark-high", "cwd": "/path", "label": "reddit-sweep"}
```

`gaal index` reads this log and resolves `caller_pid` → parent session ID (via PID→JSONL mapping from session-detect logic). Parent-child links established deterministically. Zero coordinator overhead.

**Historical sessions:** No dispatch log exists for pre-Gaal agent-mux runs. These stay unlinked. Accepted gap.

### Gaal → agent-mux: LLM-Powered Operations

Gaal dispatches to agent-mux for any operation requiring LLM inference:
- `gaal handoff` — handoff extraction
- `gaal tag --auto` — LLM-generated tagging (future)
- Smart verb modes: `--summarize`, `--rerank`, `--enrich` (future)

Configuration in `~/.gaal/config.toml`:
```toml
[llm]
default_engine = "codex"
default_model = "spark-high"

[handoff]
prompt = "~/.gaal/prompts/handoff.md"
format = "eywa"

[agent-mux]
path = "agent-mux"
```

---

## Status Determination

### Status enum: active | idle | stuck | completed | failed | unknown

| Status | Detection | Concrete signals |
|--------|-----------|-----------------|
| `active` | PID alive AND events flowing | `kill -0 <pid>` OK, `last_event_at` < 2 min ago |
| `idle` | PID alive AND events stale | `kill -0 <pid>` OK, `last_event_at` >= 2 min, < stuck threshold |
| `stuck` | PID alive AND stuck signals | Any stuck condition met |
| `completed` | PID dead AND clean exit | No PID, last JSONL: `stop_reason: "end_turn"` or `session_end` |
| `failed` | PID dead AND error exit | No PID, last JSONL: `error` type, non-zero exit, or `stop_reason: "max_tokens"` |
| `unknown` | Cannot determine | PID check fails, JSONL unreadable, insufficient data |

**Status is computed at query time, not stored.** Sessions table stores `ended_at` and `exit_signal`. If `ended_at` is NULL → probe for live PID.

### Stuck Detection — 4 signals

1. **Silence:** PID alive, `last_event_at` > 5 min ago, no permission prompt pending. Configurable via `GAAL_STUCK_SILENCE_SECS` (default 300).
2. **Loop:** Last 6 actions have ≤2 unique `(action_kind, subject)` tuples. Rolling hash detection.
3. **Context exhaustion:** Token usage > 95% of model context window.
4. **Permission-blocked:** Last JSONL record is `tool_use` with no subsequent `tool_result` AND PID alive.

`--stuck` on `gaal ls` = stuck + long idle. Sessions that need human attention.

---

## Data Model

### 4 tables: sessions, facts, handoffs, session_tags

```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    engine TEXT NOT NULL CHECK(engine IN ('claude', 'codex')),
    model TEXT,
    cwd TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    exit_signal TEXT,              -- end_turn, max_tokens, error, killed, NULL (active)
    last_event_at TEXT,
    parent_id TEXT REFERENCES sessions(id),
    jsonl_path TEXT NOT NULL,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_tools INTEGER DEFAULT 0,
    total_turns INTEGER DEFAULT 0,
    last_indexed_offset INTEGER DEFAULT 0
);

CREATE TABLE facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    ts TEXT NOT NULL,
    turn_number INTEGER,
    fact_type TEXT NOT NULL CHECK(fact_type IN (
        'file_read', 'file_write', 'command', 'error',
        'git_op', 'user_prompt', 'assistant_reply', 'task_spawn'
    )),
    subject TEXT,                  -- file path for file ops, command summary for commands
    detail TEXT,                   -- full command text, error message, etc.
    exit_code INTEGER,
    success INTEGER               -- 1/0
);

CREATE TABLE handoffs (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id),
    headline TEXT,
    projects TEXT,                 -- JSON array
    keywords TEXT,                 -- JSON array
    substance INTEGER DEFAULT 0,
    duration_minutes INTEGER DEFAULT 0,
    generated_at TEXT,
    generated_by TEXT,             -- engine/model that generated this handoff
    content_path TEXT              -- path to handoff MD in ~/.gaal/data/
);

CREATE TABLE session_tags (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    tag TEXT NOT NULL,
    PRIMARY KEY (session_id, tag)
);

-- Indexes
CREATE INDEX idx_facts_session_ts ON facts(session_id, ts);
CREATE INDEX idx_facts_type_ts ON facts(fact_type, ts);
CREATE INDEX idx_facts_subject ON facts(subject);
CREATE INDEX idx_sessions_parent ON sessions(parent_id);
CREATE INDEX idx_sessions_started ON sessions(started_at);
CREATE INDEX idx_sessions_cwd ON sessions(cwd);
CREATE INDEX idx_sessions_engine ON sessions(engine);
CREATE INDEX idx_handoffs_substance ON handoffs(substance);
CREATE INDEX idx_tags_tag ON session_tags(tag);
```

---

## Design Principles

1. **JSON default output.** Human-readable via `-H`. The output schema IS the API. Consistent across all 9 verbs.
2. **Progressive disclosure.** `ls` → `show` → `show --trace` → `--source`. Agent reads minimum needed.
3. **Semantic exit codes.** 0=success, 1=no results, 2=ambiguous, 3=not found, 10=no index, 11=parse error.
4. **Engine-agnostic.** Claude Code + Codex CLI now. New engine = one parser + one directory.
5. **Composable.** Pipe-friendly. `gaal ls --since today | jq -r '.[].id' | xargs -I{} gaal show {} --files write`
6. **Gaal owns its data.** `~/.gaal/` is the single root. SQLite, Tantivy, session MDs, handoff MDs, config, prompts — all under one tree.
7. **Batch-friendly.** `--ids`, `--tag`, `--active` flags avoid N+1 query patterns. One call, all results.
8. **LLM-powered where it matters.** Handoff generation, tagging, enrichment dispatch to agent-mux. Engine/model/prompt configurable. Deterministic operations never require LLM.

---

## Phase 2 Operations (not yet designed)

```
gaal stats --since today             # richer analytics: per-model, per-project, trends
gaal tail [id]                       # real-time event streaming (needs file watcher daemon)
gaal timeline --at <timestamp>       # what was happening across sessions at this moment?
gaal diff <id-a> <id-b>             # what changed between two sessions?
```

---

## Existing Assets

- **Orac Rust codebase**: 7k lines. ~55% (parsers, discovery, model) directly useful. Claude + Codex JSONL parsers are solid.
- **RTK** (`github.com/rtk-ai/rtk`): Rust. Session JSONL parser (`ClaudeProvider`), command classification registry, error type detection. Portable code for engine adapter + command clustering.
- **session_to_markdown.py** (`lib/sessions-tooling/`): JSONL → markdown. Reference for MD generation — Gaal's indexer absorbs this logic.
- **session-detect skill**: PID → JSONL → session mapping. Powers `gaal active`.
- **eywa**: Current handoff system. `gaal recall` + `gaal index import-eywa` replaces it.

## Research Artifacts

- Problem set (P1-P18) with community evidence from Reddit/Twitter sweep
- Data-first reasoning analysis — why the index-first approach won
- Operations-first reasoning analysis — verb design and composability
- Competitive landscape analysis — existing session observability tools

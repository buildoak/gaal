---
name: gaal
description: |
  Agent session observability CLI. Query, inspect, and search across Claude Code and Codex
  sessions. Fleet view, file/command attribution, live process monitoring, full-text search,
  semantic recall, and LLM-powered handoff generation. Rust binary, JSON output, pipe-friendly.
  Use when: session observability, find session, session history, who wrote/read/ran a file,
  search sessions, active sessions, session fleet view, inspect session health, handoff generation,
  recall context from past sessions, session memory, continuity, past sessions, what was I working on,
  session start, session end, historical sessions, prior context, previous session,
  eywa, session continuity, reconnect with previous sessions, get session context,
  cost tracking, stuck detection, child session visibility.
  Replaces eywa for session recall and handoff generation.
  Do NOT use for: cross-session prompt injection (use session-ctl), live JSONL tailing (not yet implemented).
---

# gaal

Agent session observability CLI. 9 verbs + 1 utility. JSON default, `-H` for tables.

## Paths

| What | Path |
|------|------|
| Binary | `gaal` (or `./target/release/gaal` if not on PATH) |
| Data root | `~/.gaal/` |
| SQLite index | `~/.gaal/index.db` |
| Config | `~/.gaal/config.toml` |
| Tantivy FTS | `~/.gaal/tantivy/` |
| Handoff prompt | `~/.gaal/prompts/handoff.md` |
| Session MDs | `~/.gaal/data/{engine}/sessions/YYYY/MM/DD/<id>.md` |
| Handoff MDs | `~/.gaal/data/{engine}/handoffs/YYYY/MM/DD/<id>.md` |

## Config Defaults (`~/.gaal/config.toml`)

| Key | Default | What |
|-----|---------|------|
| `llm.default_engine` | `codex` | Engine for handoff generation |
| `llm.default_model` | `gpt-5.3-codex-spark` | Model for handoff generation |
| `llm.timeout_secs` | `120` | LLM timeout |
| `handoff.prompt` | `prompts/handoff.md` | Extraction prompt path (relative to ~/.gaal/) |
| `handoff.format` | `eywa` | Default handoff output format |
| `agent-mux.path` | `agent-mux` | agent-mux binary path |
| `stuck.silence_secs` | `300` | Seconds before stuck detection triggers |

## Eywa Migration Map

| Eywa command | Gaal equivalent |
|-------------|----------------|
| `eywa get` | `gaal recall --format eywa` |
| `eywa get "query"` | `gaal recall "query" --format eywa` |
| `eywa get "topic" --days-back 30 --max 5` | `gaal recall "topic" --days-back 30 --limit 5 --format eywa` |
| `eywa extract` | `gaal handoff` (auto-detects current session) |
| `eywa extract <id>` | `gaal handoff <id>` |
| `eywa rebuild-index` | `gaal index backfill` |

The `--format eywa` flag produces coordinator-compatible output matching eywa's format.

## Session Protocol

### Session Start (coordinator calls this)
Retrieve context from past sessions relevant to the current task.

```bash
# Broad recall — what happened recently?
gaal recall --format eywa

# Focused recall — what do I know about topic X?
gaal recall "sorbent operations" --format eywa --limit 5

# Extended window for rare topics
gaal recall "gaussian moat" --days-back 30 --format eywa
```

The `--format eywa` flag produces output compatible with the coordinator's session context injection. Use `--format brief` for system-prompt-sized summaries when context budget is tight.

### Session End (coordinator calls this)
Persist this session's context for future recall. Two steps — summary first, then extract.

**Step 1: Write a session summary message.** Before calling `gaal handoff`, output a plain-text summary of what was accomplished in this session. This message becomes part of the JSONL transcript and gives the handoff extractor rich content to work with. Include:
- What was done (key actions, decisions, artifacts)
- What changed (files modified, configs updated, bugs fixed)
- What's still open (blockers, next steps, unfinished threads)

**Step 2: Generate handoff.**

```bash
# Auto-detect current session and generate handoff
gaal handoff

# Explicit session ID
gaal handoff <session-id>

# Generate handoffs for all recent sessions missing them
gaal handoff --batch --since 1d --dry-run   # preview first
gaal handoff --batch --since 1d             # then run
```

Handoff generation uses LLM via agent-mux. Default engine: codex spark (cheapest). Override with `--engine claude --model claude-sonnet-4-20250514` for higher quality.

**Cost awareness:** Each handoff costs ~$0.01-0.03 (spark) or ~$0.05-0.15 (claude). Batch runs multiply. Always `--dry-run` first.

### Index Freshness
Recall quality depends on indexed handoffs. Check and maintain:

```bash
# Health check — are handoffs indexed?
gaal index status -H

# Backfill missing sessions (safe, idempotent, no LLM)
gaal index backfill

# Import legacy eywa handoffs (one-time migration)
gaal index import-eywa
```

## Quick Reference

```bash
# Fleet view — recent sessions
gaal ls --limit 10 -H

# What did a session write?
gaal show latest --files write

# Who modified this file?
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d); echo "$OUTPUT" | jq '.'

# Search across all sessions
gaal search "gaussian moat" --limit 5

# What's running right now?
gaal active -H
```

## Decision Tree

| Need | Tool |
|------|------|
| Fleet overview / recent sessions | `gaal ls` |
| Drill into ONE session (files, commands, errors, tree) | `gaal show <id>` |
| What's running RIGHT NOW (live PIDs) | `gaal active` |
| Session health / stuck detection | `gaal inspect <id>` |
| "Who wrote/read/ran X?" (inverted query) | `gaal who <verb> <target>` |
| Free-text search across content | `gaal search <query>` |
| Semantic recall ("what do I know about X?") | `gaal recall [query]` |
| Generate handoff document (replaces eywa extract) | `gaal handoff <id\|today>` |
| Cross-session prompt injection | session-ctl (different tool) |

## Commands by Tier

### Fleet View
| Command | What |
|---------|------|
| `gaal ls` | List sessions (filters: `--engine`, `--since`, `--status`, `--tag`, `--stuck`) |
| `gaal ls --aggregate` | Token/cost totals instead of session list |
| `gaal ls --children` | Include child/worker sessions (excluded by default) |
| `gaal active` | Live process discovery (PIDs, CPU, memory, stuck signals) |

### Drill-Down
| Command | What |
|---------|------|
| `gaal show <id>` | Full session record |
| `gaal show <id> --files write` | Files modified by session |
| `gaal show <id> --errors` | Errors and non-zero exits |
| `gaal show <id> --tree` | Spawn hierarchy (parent/children) |
| `gaal show <id> --trace` | Full event timeline (level 2) |
| `gaal inspect <id>` | Operational snapshot (context %, velocity, stuck signals) |

### Inverted Queries
| Command | What |
|---------|------|
| `gaal who read <path>` | Sessions that read a file |
| `gaal who wrote <path>` | Sessions that modified a file |
| `gaal who ran "<cmd>"` | Sessions that ran a command |
| `gaal who touched <term>` | Broadest — files OR commands mentioning term |
| `gaal who installed <pkg>` | Package install detection (pip/npm/brew/cargo) |

### Search & Recall
| Command | What |
|---------|------|
| `gaal search "<query>"` | BM25 full-text search across session content |
| `gaal recall [query]` | Semantic session retrieval (IDF + recency scoring) |
| `gaal recall --format brief` | System-prompt-sized summaries |

### LLM-Powered
| Command | What |
|---------|------|
| `gaal handoff <id\|today>` | Generate handoff doc via agent-mux LLM dispatch |

### Index Management
| Command | What |
|---------|------|
| `gaal index status` | Index health report |
| `gaal index backfill` | Index all existing JSONL files |
| `gaal index import-eywa` | Import legacy eywa handoff-index.json |
| `gaal tag <id> "label"` | Add/remove session tags |

All verbs accept `-H` for human-readable tables. Full flag reference: `references/verb-reference.md`

## Output Contract

- **Default:** JSON to stdout. Errors to stderr as JSON `{"error": "...", "exit_code": N}`.
- **`-H` flag:** Human-readable tables.
- **Exit codes:** 0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error. Full table: `references/exit-codes.md`
- **Batch flags:** `--ids`, `--tag`, `--active` avoid N+1 patterns. One call, all results.

## Agent Consumption Notes

**Pipe gotcha with `gaal who`:** The `who` verb consumes trailing args greedily. Piping directly (`gaal who wrote X | jq`) may fail. Workaround:
```bash
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.[0].session_id'
```

**jq assertion pattern** (verify schema in scripts):
```bash
gaal ls --limit 1 | jq -e 'length == 1 and all(.id and .engine)' > /dev/null
```

**Composable pipeline:**
```bash
gaal ls --since today | jq -r '.[].id' | xargs -I{} gaal show {} --files write
```

## Anti-Patterns

| Do NOT | Do instead |
|--------|------------|
| Pipe `gaal who` directly with `\|` | Capture to variable first, then pipe |
| Use `gaal active` for "recent active sessions" | Use `gaal ls --status active` (queries archive) |
| Read entire session JSONL manually | Use `gaal show <id> --trace` or `--source` |
| Call `gaal show` in a loop for multiple IDs | Use `gaal show --ids a1b2,c3d4` (batch mode) |
| Assume `gaal recall` works without handoffs | Check `gaal index status` handoffs_total first |
| Run `gaal handoff` without agent-mux installed | Verify agent-mux availability; handoff needs LLM |

## Security / Approval

- **Read-only by default.** All verbs except `handoff` and `tag` are pure reads.
- **`handoff` dispatches to LLM** via agent-mux. Costs money. Confirm before batch runs.
- **`index backfill` is safe.** Reads JSONL, writes to `~/.gaal/`. No external calls.
- **`index import-eywa` is safe.** Copies data from eywa to gaal's own directory.

## Bundled Resources

| Path | What | When to load |
|------|------|-------------|
| `references/verb-reference.md` | Full flag + schema reference for all 10 verbs | Need exact flags, output shapes, or edge cases |
| `references/exit-codes.md` | Exit code table with agent response guidance | Handling errors in scripts or pipelines |
| `references/troubleshooting.md` | Known bugs and workarounds | Something unexpected happens |

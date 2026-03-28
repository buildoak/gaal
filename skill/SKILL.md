---
name: gaal
description: |
  Agent session observability CLI. Query, inspect, and search across Claude Code and Codex
  sessions. Fleet view, file/command attribution, full-text search, semantic recall, token/cost
  accounting, transcript rendering, and LLM-powered handoff generation. Rust binary, JSON output,
  pipe-friendly. AX-designed errors teach agents what went wrong and how to fix it.
  Use when: session observability, find session, session history, who wrote/read/ran a file,
  search sessions, session fleet view, inspect session details, handoff generation,
  recall context from past sessions, session memory, continuity, past sessions, what was I working on,
  session start, session end, historical sessions, prior context, previous session,
  eywa, session continuity, reconnect with previous sessions, get session context,
  cost tracking, token breakdown, cache tokens, self-identification, salt token, transcript.
  Replaces eywa for session recall and handoff generation.
  Do NOT use for: cross-session prompt injection (use session-ctl), live JSONL tailing (not supported),
  real-time process monitoring (removed in v0.1.0).
---

# gaal

Agent session observability CLI. 11 commands. JSON default, `-H` for human-readable tables and errors.

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

## Eywa Migration Map

| Eywa command | Gaal equivalent |
|-------------|----------------|
| `eywa get` | `gaal recall --format eywa` |
| `eywa get "query"` | `gaal recall "query" --format eywa` |
| `eywa get "topic" --days-back 30 --max 5` | `gaal recall "topic" --days-back 30 --limit 5 --format eywa` |
| `eywa extract` | `gaal create-handoff` (auto-detects current session) |
| `eywa extract <id>` | `gaal create-handoff <id>` |
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

**Step 1: Write a session summary message.** Before calling `gaal create-handoff`, output a plain-text summary of what was accomplished in this session. This message becomes part of the JSONL transcript and gives the handoff extractor rich content to work with. Include:
- What was done (key actions, decisions, artifacts)
- What changed (files modified, configs updated, bugs fixed)
- What's still open (blockers, next steps, unfinished threads)

**Step 2: Generate handoff.**

```bash
# Auto-detect current session and generate handoff
gaal create-handoff

# Explicit session ID
gaal create-handoff <session-id>

# Generate handoffs for all recent sessions missing them
gaal create-handoff --batch --since 1d --dry-run   # preview first
gaal create-handoff --batch --since 1d             # then run
```

Handoff generation uses LLM via agent-mux. Default engine: codex spark (cheapest). Override with `--engine claude --model claude-sonnet-4-20250514` for higher quality.

**Cost awareness:** Each handoff costs ~$0.01-0.03 (spark) or ~$0.05-0.15 (claude). Batch runs multiply. Always `--dry-run` first.

### Self-Handoff Protocol (from inside a running session)

When a session needs to generate its own handoff — without knowing its session ID or relying on process detection — use the salt-based self-identification flow.

**Why this exists:** `gaal create-handoff` normally takes a session ID. But a session doesn't know its own ID. The salt protocol solves this: embed a unique token into the JSONL transcript, then search for it to find the file path.

```bash
# Step 1: Generate + embed salt (MUST be its own Bash tool call)
SALT=$(gaal salt)
echo "$SALT"

# Step 2: Find own JSONL (MUST be a SEPARATE Bash tool call — flush must happen first)
JSONL=$(gaal find-salt "$SALT" | jq -r .jsonl_path)

# Step 3: Generate handoff from the discovered JSONL
gaal create-handoff --jsonl "$JSONL"
```

**Critical: Steps 1 and 2 MUST be separate Bash tool invocations.** The salt appears in the tool-result of step 1, and Claude Code flushes it to the JSONL between tool calls. `gaal find-salt` in step 2 scans JSONL files for that salt. If you chain them with `&&` in a single call, the salt won't be flushed yet and `gaal find-salt` will return nothing.

**How it works under the hood:**
1. `gaal salt` generates a unique token (e.g., `gaal:salt:a7f3b2c1`) and prints it to stdout
2. The token lands in the JSONL as part of the tool-result when Claude Code records this Bash call
3. `gaal find-salt <SALT>` greps all recent JSONL files for the token and returns the matching file path
4. `gaal create-handoff --jsonl <path>` generates a handoff directly from the JSONL file, bypassing ID resolution

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
gaal inspect latest --files write

# Who modified this file?
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d); echo "$OUTPUT" | jq '.'

# Search across all sessions
gaal search "gaussian moat" --limit 5

# Session transcript artifact (path metadata by default)
gaal transcript latest

# Dump transcript markdown directly
gaal transcript latest --stdout
```

## Decision Tree

| Need | Tool |
|------|------|
| Fleet overview / recent sessions | `gaal ls` |
| Drill into ONE session (files, commands, errors, trace, source path) | `gaal inspect <id>` |
| Get rendered transcript markdown for ONE session | `gaal transcript <id>` |
| Fleet totals instead of individual sessions | `gaal ls --aggregate` |
| Session health / operational snapshot | `gaal inspect <id>` |
| "Who wrote/read/ran X?" (inverted query) | `gaal who <verb> <target>` |
| Free-text search across content | `gaal search <query>` |
| Semantic recall ("what do I know about X?") | `gaal recall [query]` |
| Generate handoff document (replaces eywa extract) | `gaal create-handoff <id\|today>` |
| Self-identify current session (from inside) | `gaal salt` → `gaal find-salt <SALT>` → `gaal create-handoff --jsonl` |
| Cross-session prompt injection | session-ctl (different tool) |

## Commands by Tier

### Fleet View
| Command | What |
|---------|------|
| `gaal ls` | List sessions (filters: `--engine`, `--since`, `--before`, `--cwd`, `--tag`, `--sort`). Default limit 10, shows "N of M" footer |
| `gaal ls --aggregate` | Token/cost totals instead of session list |
| `gaal ls --all` | Include noisy sessions (0 tool calls and <30s duration) |

### Drill-Down
| Command | What |
|---------|------|
| `gaal inspect <id>` | Session details with optional focused views. `-H` renders human-readable output |
| `gaal inspect <id> --files write` | Files modified by session |
| `gaal inspect <id> --errors` | Errors and non-zero exits |
| `gaal inspect <id> --commands` | Commands only |
| `gaal inspect <id> --trace` | Full event timeline |
| `gaal inspect <id> --source` | Raw JSONL source path |
| `gaal inspect --ids a1b2,c3d4` | Batch inspect multiple sessions |
| `gaal transcript <id>` | Return transcript path/size metadata as JSON |
| `gaal transcript <id> --stdout` | Dump rendered transcript markdown to stdout |

### Inverted Queries
| Command | What |
|---------|------|
| `gaal who read <path>` | Sessions that read a file |
| `gaal who wrote <path>` | Sessions that modified a file |
| `gaal who ran "<cmd>"` | Sessions that ran a command |
| `gaal who touched <term>` | Broadest — files OR commands mentioning term |
| `gaal who changed <path>` | Sessions that changed a file |
| `gaal who deleted <path>` | Sessions that deleted a file or removed it via command |

All `who` verbs: default limit 10 with "showing N of M" indicator. Add `-F`/`--full` for verbose per-fact output. Verb matching is by command name (not substring). Search window displayed in output.

### Search & Recall
| Command | What |
|---------|------|
| `gaal search "<query>"` | BM25 full-text search across session content |
| `gaal recall [query]` | Semantic session retrieval (IDF + recency scoring) |
| `gaal recall --format brief` | System-prompt-sized summaries |

### LLM-Powered
| Command | What |
|---------|------|
| `gaal create-handoff <id\|today>` | Generate handoff doc via agent-mux LLM dispatch |

### Self-Identification
| Command | What |
|---------|------|
| `gaal salt` | Generate unique salt token for self-identification (embed in JSONL via tool-result) |
| `gaal find-salt <SALT>` | Find JSONL file containing a salt token (returns `jsonl_path`, `engine`, `session_id`) |

### Index Management
| Command | What |
|---------|------|
| `gaal index status` | Index health report |
| `gaal index backfill` | Index all existing JSONL files |
| `gaal index import-eywa` | Import legacy eywa handoff-index.json |
| `gaal tag <id> "label"` | Add/remove session tags |

All commands default to brief/summary output (agent-optimized). Use `--full` / `-F` for verbose output. All accept `-H` for human-readable tables. Full flag reference: `references/verb-reference.md`

## Session ID Resolution

Gaal stores **shortened 8-character IDs**, not full UUIDs. The shortening logic differs by engine:

| Engine | UUID type | Short ID derivation | Example |
|--------|-----------|--------------------:|---------|
| Claude | UUIDv4 (`2c47d1f0-6773-4587-b487-c205abca8f0a`) | **First 8 chars** | `2c47d1f0` |
| Codex | UUIDv7 (`019cd256-c7f9-72f0-a2fe-924fe3e8c603`) | **Last 8 hex chars** (dashes stripped) | `e3e8c603` |

**Why different?** UUIDv4 is random throughout — first 8 chars are unique. UUIDv7 has a shared timestamp prefix (sessions started in the same ms share it) — only the random suffix provides uniqueness, so last 8 hex chars are used.

**What you can pass to gaal commands** (`inspect`, `transcript`, `create-handoff`, etc.):

| Input | Behavior |
|-------|----------|
| Full UUID (`2c47d1f0-6773-4587-...`) | Gaal truncates internally to short ID |
| Short 8-char ID (`2c47d1f0`) | Used directly for DB lookup |
| Prefix of short ID (`2c47`) | Prefix match via SQL `LIKE` — resolves if unique, exit code 2 if ambiguous |
| `latest` | Resolves to most recent session |

**Exit codes for resolution:** 0=found, 2=ambiguous prefix (multiple matches), 3=not found.

## Output Contract

- **Default:** JSON to stdout. Errors to stderr as JSON `{"error": "...", "exit_code": N}`.
- **`-H` flag:** Human-readable tables. Also applies to error output — errors render as structured text (what/example/hint) instead of JSON.
- **Exit codes:** 0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error. Full table: `references/exit-codes.md`
- **Batch flags:** `--ids` and `--tag` avoid N+1 patterns where supported. One call, all results.

## AX Error Handling

Every gaal error is designed to teach calling agents (AX = Agent Experience). Errors include three parts:
1. **What went wrong** — specific to the command and input
2. **A working example** — correct invocation the agent can copy-paste
3. **A hint** — what to try next to recover

When handling gaal errors in scripts or agent code:
- Check exit code first (0=success, non-zero=error)
- Parse stderr for structured error info
- With `-H`: errors are plain text (What/Example/Hint format)
- Without `-H`: errors are JSON `{"error": "...", "exit_code": N}`
- Common recovery: widen `--since` window, check session ID exists via `gaal ls`, run `gaal index backfill` if index is stale

## Agent Consumption Notes

**Pipe gotcha with `gaal who`:** The `who` verb consumes trailing args greedily. Piping directly (`gaal who wrote X | jq`) may fail. Workaround:
```bash
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.results[0].session_id // .[0].session_id'
```

**jq assertion pattern** (verify schema in scripts):
```bash
gaal ls --limit 1 | jq -e '.sessions | length == 1 and all(.[]; .id and .engine)' > /dev/null
```

**Composable pipeline:**
```bash
gaal ls --since today | jq -r '.sessions[].id' | xargs -I{} gaal inspect {} --files write
```

**Transcript behavior:** `gaal transcript <id>` is path-first by default. It returns JSON with the rendered markdown path, size, and token estimate. Use `--stdout` only when you explicitly want the markdown content in the calling context.

## Anti-Patterns

| Do NOT | Do instead |
|--------|------------|
| Pipe `gaal who` directly with `\|` | Capture to variable first, then pipe |
| Assume `gaal ls` has `--status active` or a separate `active` command | Use `gaal ls --all` plus `gaal inspect <id>` for drill-down |
| Read entire session JSONL manually | Use `gaal inspect <id> --trace`, `gaal inspect <id> --source`, or `gaal transcript <id>` |
| Call `gaal inspect` in a loop for multiple IDs | Use `gaal inspect --ids a1b2,c3d4` where possible |
| Assume `gaal recall` works without handoffs | Check `gaal index status` handoffs_total first |
| Run `gaal create-handoff` without agent-mux installed | Verify agent-mux availability; handoff needs LLM |

## Security / Approval

- **Read-only by default.** All verbs except `create-handoff` and `tag` are pure reads.
- **`create-handoff` dispatches to LLM** via agent-mux. Costs money. Confirm before batch runs.
- **`index backfill` is safe.** Reads JSONL, writes to `~/.gaal/`. No external calls.
- **`index import-eywa` is safe.** Copies data from eywa to gaal's own directory.

## Bundled Resources

| Path | What | When to load |
|------|------|-------------|
| `references/verb-reference.md` | Full flag + schema reference for all 12 commands | Need exact flags, output shapes, or edge cases |
| `references/exit-codes.md` | Exit code table with agent response guidance | Handling errors in scripts or pipelines |
| `references/troubleshooting.md` | Known bugs and workarounds | Something unexpected happens |

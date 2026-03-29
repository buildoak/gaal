# gaal

Session observability for AI coding agents. Like k9s for your Claude Code and Codex fleet.

Full documentation lives in [`docs/`](./docs/README.md). Use this README as the short overview and `docs/` for architecture, command reference, agent guide, and format details.

## The problem

You're running Claude Code and Codex sessions all day. Some last minutes, some last hours. You need to know: what did that session from yesterday actually do? Where's the context I need to continue this work? How much did it cost?

The JSONL files are there -- Claude and Codex both write them -- but they're 10-50MB blobs of undocumented, engine-specific event streams. Nobody reads those raw. gaal parses both formats, indexes everything into SQLite + Tantivy, and gives you five-second answers to the questions that actually matter.

## Install

```bash
git clone https://github.com/buildoak/gaal && cd gaal
cargo install --path .
```

Single binary. No runtime dependencies. macOS-first (Linux possible but untested).

Then index your existing sessions:

```bash
gaal index backfill
```

## Quick start

Three commands you'll reach for daily:

**Fleet view -- what happened recently?**

```
$ gaal ls --limit 5 -H
ID        Engine  Started      Duration  Tokens       Peak  Tools  Model              CWD
--------  ------  -----------  --------  -----------  ----  -----  -----------------  -----------
acabe588  claude  today 18:25  3h 46m    2K / 13K     124K  74     claude-opus-4-6    coordinator
1ab21f89  claude  today 18:38  3h 25m    28 / 311     74K   8      claude-opus-4-6
65eeec4f  claude  today 19:03  2m 1s     2K / 495     69K   10     claude-sonnet-4-6
875f36ae  codex   today 21:50  8m 39s    180K / 21K   181K  114    gpt-5.4            gaal
```

**Drill into a session:**

```
$ gaal inspect latest --tokens -H
Session: 875f36ae (codex, gpt-5.4)
Duration: 8m 39s
Tokens: input=180K output=21K cache_read=5.4M
Peak context: 181K
Estimated cost: $1.23
Tools used: 114
```

**Find past sessions about a topic:**

```
$ gaal recall "auth refactor" --format brief --limit 3 -H
session: 2b2f6f7e
date: 2026-03-04
headline: Refactored auth middleware, added refresh token handling
projects: gaal, coordinator
substance: 2 duration_minutes: 5290 score: 29.923
```

## Commands

| Command | What it does |
|---------|-------------|
| `ls` | Fleet view -- all sessions, filterable by engine/date/cwd/tag. |
| `inspect <id>` | Session detail view. Files, commands, errors, tokens, git ops. |
| `transcript <id>` | Session transcript markdown -- path metadata or `--stdout` dump. |
| `who <verb> <target>` | Inverted query: which session read/wrote/ran/deleted X? |
| `search <query>` | Full-text search via Tantivy BM25. Filter by field, engine, time. |
| `recall <topic>` | Ranked retrieval for session continuity. Best sessions first. |
| `create-handoff [id]` | Generate handoff document via LLM extraction (agent-mux). |
| `salt` | Generate a salt token for self-identification (see below). |
| `find-salt <salt>` | Find the JSONL file containing a salt token. |
| `tag <id> <tags>` | Apply or remove tags on sessions. |
| `index` | Index maintenance -- backfill, status, import-eywa. |

All commands output JSON by default. Add `-H` for human-readable tables.

## How it works

The indexer parses both Claude and Codex JSONL formats -- they have fundamentally different event schemas -- into a unified SQLite store with Tantivy full-text search on top. Data lives at `~/.gaal/`.

**Token accounting** tracks input, output, cache_read, cache_creation, and reasoning tokens per session. Peak context = max(input + cache_read + cache_creation) across all turns. Cost estimation is model-aware (Opus/Sonnet/Codex rates).

**Session detection** from inside a running session uses content-addressed salt tokens -- see [self-handoff protocol](#self-handoff-protocol).

## Agent integration

gaal's primary consumers are AI agents, not humans. JSON output by default. A typical agent retrieval:

```bash
# "What happened in the last 2 weeks on this topic?"
gaal recall "auth refactor" --format brief --limit 3

# "Which session touched this file?"
gaal who wrote "src/auth/middleware.rs" --since 7d

# "Fleet status"
gaal ls --since 1d -H
```

Agents get ~500-token summaries, not 26K JSONL dumps. The `--full` / `-F` flag unlocks verbose output when needed.

### Error handling (AX design)

Every gaal error is designed to teach calling agents:

1. **What went wrong** -- specific, actionable
2. **A working example** -- correct invocation the agent can copy
3. **A hint** -- what to try next

```
$ gaal inspect nonexistent -H
What went wrong: Session `nonexistent` was not found.
Example: gaal inspect latest -H
Hint: List recent sessions with `gaal ls --since 7d -H`, then rerun `gaal inspect` with a valid 8-character ID prefix.
```

Exit codes: 0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error.

### Recall

The primary retrieval interface for agents continuing previous work:

```
$ gaal recall "session detection" --format brief --limit 2 -H
session: 2b2f6f7e
date: 2026-03-04
headline: Built gaal as eywa replacement, shipped cutover-ready
projects: gaal, coordinator, eywa-continuum
substance: 2 duration_minutes: 5290 score: 29.923
```

Formats: `brief` (default, agent-optimized), `summary`, `handoff`, `full`, `eywa` (legacy compat).

### Token breakdown

```bash
gaal inspect latest --tokens
```

Returns input_total, output_total, cache_read, cache_creation, reasoning_tokens, estimated_cost_usd, turns, and per-turn averages. Cache tokens are critical for Opus sessions where 90%+ of input is cache reads.

### Transcript

```bash
# Get transcript path metadata (default)
gaal transcript latest

# Dump markdown to stdout (use sparingly -- can be 50K+ tokens)
gaal transcript latest --stdout

# Force re-render even if cached
gaal transcript latest --force --stdout
```

Transcript frontmatter includes: session_id, date, model (full name), turns, all four token fields (input, output, cache_read, cache_creation).

## Self-handoff protocol

The killer feature. An agent finds its own session and generates a handoff for continuity -- no PIDs, no process trees, works through subagent indirection and concurrent sessions.

```bash
# Step 1: Generate and embed a salt token (prints to JSONL via tool-result)
SALT=$(gaal salt)
echo "$SALT"

# Step 2: Find own JSONL file (MUST be a separate tool call -- JSONL flushes between calls)
JSONL=$(gaal find-salt "$SALT" | jq -r .jsonl_path)

# Step 3: Generate handoff
gaal create-handoff --jsonl "$JSONL"
```

Steps 1 and 2 **must** be separate tool invocations. The salt appears in the tool-result of step 1, and `gaal find-salt` scans for it in step 2. The JSONL flush happens between calls.

Why this matters: when an agent runs inside a subagent tree (Task -> agent-mux -> Codex worker), PID-based detection breaks. The salt is content-addressed -- it doesn't care about process ancestry.

## Build

```bash
cargo build --release
```

Always `--release`. The installed binary is a symlink to `target/release/gaal`. Debug builds don't update it.

Clean build: ~8 min. Incremental: ~30s.

## What gaal does not do

gaal is session observability. Not a session manager, not a debugger, not an agent framework.

Killed features (not deferred -- deleted):
- **`gaal active` command** -- process monitoring too fragile
- **`gaal show` command** -- merged into `inspect`
- **Stuck detection** -- heuristic garbage, wrong more than right
- **Parent-child linking** -- 1 out of 2,433 sessions ever linked
- **Loop detection** -- insufficient signal in JSONL

## License

[MIT](./LICENSE)

Built by [Nick Oak](https://github.com/buildoak).

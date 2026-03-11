# gaal

Session observability for AI coding agents. Like k9s for your Claude Code and Codex fleet.

## The problem

You're running Claude Code and Codex sessions all day. Some last minutes, some last hours. You need to know: what's running right now? What did that session from yesterday actually do? Where's the context I need to continue this work?

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

**What's running right now?**

```
$ gaal active -H
FLEET: 2 active, 1 starting

3ffb65d8  codex   starting   1d 3h  "coordinator"
58ec9fa4  claude  active    21h 9m  "boot Jenkins"  [3 PIDs]
019cdbf6  codex   active     5m 5s  "coordinator"
```

**What did session X do?**

```
$ gaal show latest -H
ID: 8041767e
Engine: codex
Model: gpt-5.4
Status: completed
Duration: 249s
Tokens: in=656K out=12K
Tools: 30
Commands:
  $ cat SOUL.md (exit 0)
  $ rg -n "sqrt36-upper-bound" ... (exit 0)
  ...
```

**Find past sessions about a topic:**

```
$ gaal search "handoff" --limit 3 -H
Score  Session   Engine  Turn  Type     Snippet
12.62  466b3aac  codex   1     command  rg -n "handoff|extract.*handoff|..." ...
12.60  0e2361ff  codex   1     error    let args = gaal::commands::handoff::HandoffArgs ...
12.56  b47749ac  codex   1     error    handoff generates handoff MDs via LLM ...
```

## Commands

| Command | What it does |
|---------|-------------|
| `active` | Live process discovery. PIDs, engine, duration, CWD, last action. |
| `ls` | Fleet view -- all sessions, filterable by status/engine/date/cwd/tag. |
| `show <id>` | Full session record. Files, commands, errors, tokens, git ops. |
| `inspect <id>` | Operational snapshot -- CPU, RSS, velocity, context window usage. |
| `who <verb> <target>` | Inverted query: which session read/wrote/ran/deleted X? |
| `search <query>` | Full-text search via Tantivy BM25. Filter by field, engine, time. |
| `recall <topic>` | Ranked retrieval for session continuity. Best sessions first. |
| `handoff <id>` | Generate handoff document via LLM extraction. |
| `salt` | Generate a salt token for self-identification (see below). |
| `find <salt>` | Find the JSONL file containing a salt token. |
| `tag <id> <tags>` | Apply or remove tags on sessions. |
| `index` | Index maintenance -- backfill, reindex, prune, status. |

All commands output JSON by default. Add `-H` for human-readable tables.

## How it works

gaal discovers sessions through two paths. **From outside**: `proc_pidpath` via macOS libproc FFI resolves running Claude/Codex processes, cross-references with JSONL file discovery, and maps PIDs to sessions. **From inside** (when an agent wants to find its own session): content-addressed salt tokens -- see [self-handoff protocol](#self-handoff-protocol).

The indexer parses both Claude and Codex JSONL formats -- they have fundamentally different event schemas -- into a unified SQLite store with Tantivy full-text search on top. Data lives at `~/.gaal/`.

## Agent integration

gaal's primary consumers are AI agents, not humans. JSON output by default. A typical agent retrieval:

```bash
# "What happened in the last 2 weeks on this topic?"
gaal recall "auth refactor" --format brief --limit 3

# "Which session touched this file?"
gaal who wrote "src/auth/middleware.rs" --since 7d

# "Fleet status for the coordinator"
gaal active
```

Agents get ~500-token summaries, not 26K JSONL dumps. The `--full` / `-F` flag unlocks verbose output when needed.

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

### Inspect

Real-time operational snapshot of a running session:

```
$ gaal inspect latest -H
ID:      019cdbf6
Engine:  codex
Status:  active
PID:     38425
Uptime:  5m 23s
CPU:     0.0%
RSS:     82.8 MB
Tokens:  total=668K ctx_window=134K ctx_limit=128K
Velocity: 0.0 actions/min | 240K tokens/min (5m window)
```

`--watch` for live 2s polling. `--active` to inspect all running sessions at once.

### Fleet view

```
$ gaal ls --limit 5 -H
ID        Engine  Status     Started      Duration  Tokens      Tools  Model             CWD
8041767e  codex   completed  today 12:14  4m 9s     656K / 12K  30     gpt-5.4           /.../coordinator
80db6eaa  codex   completed  today 11:51  20m 17s   7.2M / 29K  115    gpt-5.4           /.../solver
b6ed94fa  claude  completed  today 11:30  45m 7s    13 / 10     7      claude-opus-4-6   /.../coordinator
```

Filter by anything: `--engine claude`, `--status active`, `--since 1d`, `--cwd /path`, `--tag important`. Sort by `--sort tokens` or `--sort cost`.

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
- **Stuck detection** -- heuristic garbage, wrong more than right
- **Context % calculation** -- always wrong across engines
- **Parent-child linking** -- 1 out of 2,433 sessions ever linked
- **Loop detection** -- insufficient signal in JSONL

## License

[MIT](./LICENSE)

Built by [Nick Oak](https://github.com/buildoak).

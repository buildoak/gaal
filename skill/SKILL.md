---
name: gaal
description: |
  Agent session observability CLI — turns AI coding sessions into first-class queryable artifacts.
  Parses Claude Code + Codex JSONL, indexes into SQLite + Tantivy FTS, answers questions in seconds.
  Use when: recalling past context, attributing file changes, searching session history, generating
  continuity handoffs, inspecting sessions, viewing transcripts, managing session taxonomy,
  session search, who wrote, who read, who ran, fleet view, inspect session, recall context,
  transcript, tag management, self-identification, salt token, subagents, token accounting,
  cache tokens, continuity, prior context, attribution, handoff.
  11 commands. JSON default, -H for human. AX errors teach agents what went wrong + how to recover.
---

# gaal

Session observability for AI coding agents.

Claude Code and Codex emit JSONL session logs — 10-50MB blobs of undocumented, engine-specific event streams. Raw, they're useless. Gaal parses both formats, indexes them into SQLite and Tantivy, and turns raw traces into answers.

The core idea: **sessions are first-class queryable artifacts, not throwaway logs.**

Every session you ran is indexed, searchable, and summarizable. Use gaal to resume work, attribute changes, search across sessions, and maintain continuity across context resets. It is the memory layer that makes each new session less amnesic than the last.

## Design Principles

**1. Two-source architecture.** Database for speed, filesystem for truth. SQLite powers `ls`, `inspect`, `who`, `recall`, `search` — fast structured queries over session metadata and extracted facts. Raw JSONL files on disk power `transcript` rendering, `salt` scanning, and subagent discovery. Parent JSONL `toolUseResult` blocks give fleet-level subagent metadata (tokens, duration, status). Subagent JSONL files give the full turn-by-turn trace. Both sources needed. The DB is derived from the files, not the other way around.

**2. Errors teach.** Every gaal error has three parts: what went wrong, a working example, and a hint for the next action. This is the AX principle — agent experience matters more than API surface. A failed command should be productive: parse the error, extract the example, retry. Zero cryptic failures is the bar.

**3. Agent-native.** JSON by default. Exit codes are stable contracts (0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error). Composable with jq, pipeable into scripts, assertable with `jq -e`. `-H` is the human escape hatch, not the default.

**4. Taxonomy over heuristics.** Sessions have types — `standalone`, `coordinator`, `subagent` — not guessed states. We killed stuck detection, loop detection, velocity estimation, and process monitoring because heuristics lie more often than they help. What survived: deterministic classification based on observable structure.

**5. Evidence first.** When working on gaal's own code: grep real JSONL before reasoning about schemas. When using gaal: trust `inspect --trace` output over assumptions about what a session contains. Ground truth is sacred — confident lies compound.

## The Six Questions

Every gaal command answers exactly one question:

| Question | Command |
|----------|---------|
| What sessions exist? | `gaal ls` |
| What happened in this session? | `gaal inspect <id>` |
| Which sessions touched this? | `gaal who <verb> <target>` |
| Where does this text appear? | `gaal search <query>` |
| What past context is relevant? | `gaal recall [topic]` |
| How do I create continuity? | `gaal create-handoff` |

Supporting commands: `transcript` (rendered markdown of a session), `salt`/`find-salt` (self-identification), `index` (maintenance), `tag` (annotation).

If you're unsure which command to reach for, match your intent to one of these six questions.

## Quick Start

```bash
gaal ls -H                              # fleet overview
gaal inspect latest --tokens -H         # drill into newest session
gaal who wrote CLAUDE.md                # attribution
gaal search "auth refactor" --limit 5   # free-text search
gaal recall --format brief              # continuity recall
gaal create-handoff latest              # generate handoff
```

## Session Taxonomy

```
standalone    — normal session, no subagents
coordinator   — parent that spawned subagents via Agent tool
subagent      — child spawned by a coordinator
```

`gaal ls` hides subagents by default. `--include-subagents` or `--session-type subagent` to surface them. `inspect` shows a Subagents table for coordinators, parent linkage for subagents. Attribution via `who` flows through the chain — if a subagent wrote a file, gaal traces it back through the parent.

## Continuity Protocol

**Start of session** — pull relevant context:
```bash
gaal recall "topic" --format brief --limit 5
```

**End of session** — generate a handoff for the next agent:
```bash
gaal create-handoff
```

Recall quality depends on indexed handoffs. If recall returns nothing useful, check `gaal index status` — you may need `gaal index backfill`.

## Self-Identification

When an agent needs to identify its own running session:

```bash
SALT=$(gaal salt)
echo "$SALT"
JSONL=$(gaal find-salt "$SALT" | jq -r .jsonl_path)
gaal create-handoff --jsonl "$JSONL"
```

`salt` and `find-salt` must be separate tool calls — the JSONL must flush between them.

Fallback if salt scanning fails (sandbox environments, unflushed logs): `gaal inspect latest --source` gives the most recent session's JSONL path.

## Essential Patterns

**Pipe gotcha:** `who` consumes trailing args greedily. Capture first, then pipe:
```bash
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.'
```

**Assertion gate:**
```bash
gaal ls --limit 1 | jq -e '.sessions | length == 1' > /dev/null
```

**Composable pipeline:**
```bash
gaal ls --since today | jq -r '.sessions[].id' | xargs -I{} gaal inspect {} --files write
```

**Transcript:** `gaal transcript <id>` returns path metadata by default. Use `--stdout` only when you want markdown content in the calling context.

## What Gaal Does Not Do

- Real-time monitoring of active processes (killed — too fragile)
- Stuck detection or loop detection (killed — heuristics lie)
- Velocity or context-percentage estimates (killed — unreliable)
- Cross-session prompt injection (use `session-ctl`)
- Live JSONL tailing (not a daemon, not a watcher)

These were deliberately removed, not deferred.

## Admin: recover-orphans

`gaal index recover-orphans` — one-off subcommand to recover subagent JSONL files orphaned by Claude Code's 30-day cleanup. Creates ghost parent records tagged `_recovered`. Run with `--dry-run` first. Not part of normal workflow.

## Hard Rules

- **Read-only by default.** Most gaal commands are pure queries. Commands that mutate state: `create-handoff`, `index backfill`, `index reindex`, `index prune`, `index import-eywa`, `index recover-orphans`, `tag`. Do not call mutation commands without explicit task need — they change the DB or dispatch LLM calls.
- **`create-handoff` costs money.** It dispatches to an external LLM via agent-mux. Always `--dry-run` first for batch operations. One careless loop can burn real dollars.
- **When developing gaal itself:** always `cargo build --release`. Debug builds don't update the installed binary (symlinked to `target/release/gaal`). Read gaal's own CLAUDE.md before writing code — it has Rust conventions and test contracts.
- **Evidence first.** Grep real JSONL before reasoning about schemas. Don't guess field names — Claude Code and Codex emit different event shapes. Confident guesses about JSONL structure are the #1 source of gaal bugs.
- **Trust gaal's JSON output, not raw JSONL parsing.** Gaal normalizes two incompatible formats into one clean schema. If you're tempted to read a session JSONL directly, you're using the wrong command.

## Anti-Patterns

| Do NOT | Do Instead | Why |
|--------|------------|-----|
| Pipe `gaal who` directly with `\|` | Capture to variable first | `who` consumes trailing args greedily — the pipe target becomes an argument |
| Assume `gaal ls` has `--status active` | Use `gaal ls --all` + `gaal inspect <id>` | Active monitoring was killed — heuristics lie. Taxonomy is deterministic, status was not |
| Read raw session JSONL manually | Use `gaal inspect --trace` or `gaal transcript` | Gaal normalizes dual formats; raw parsing will silently break on the other engine's schema |
| Call `gaal inspect` in a loop | Use `gaal inspect --ids a1b2,c3d4` | Batch mode exists — saves N-1 DB opens and N-1 process spawns |
| Assume `gaal recall` works without handoffs | Check `gaal index status` first | Recall queries the handoff index, not raw sessions. No handoffs = no recall results |
| Run `gaal create-handoff` without checking agent-mux | Verify `agent-mux` is available | Handoff generation dispatches to an external LLM — if agent-mux is down, the command hangs |
| Use `cargo build` (debug) when developing gaal | Always `cargo build --release` | Installed binary is symlinked to release target. Debug build compiles but changes nothing |

## Reference

Full command flags, output schemas, format comparison tables, and operational details:

| Need | Where | Read when |
|------|-------|-----------|
| Agent consumption guide | [docs/agent-guide.md](/Users/otonashi/thinking/building/gaal/docs/agent-guide.md) | First time using gaal in a pipeline, or hitting unexpected output shapes |
| Command reference by group | [docs/commands/](/Users/otonashi/thinking/building/gaal/docs/commands/) | Need exact flags, output schemas, or edge-case behavior for a specific command |
| Output formats and exit codes | [docs/formats.md](/Users/otonashi/thinking/building/gaal/docs/formats.md) | Choosing between recall formats (brief/summary/handoff/full/eywa) or handling non-zero exits |
| Architecture deep-dive | [docs/architecture.md](/Users/otonashi/thinking/building/gaal/docs/architecture.md) | Building on gaal's internals, extending the parser, or understanding the two-source model |
| Eywa migration (legacy) | [docs/migration.md](/Users/otonashi/thinking/building/gaal/docs/migration.md) | One-time: migrating from eywa to gaal. Not needed for normal operation |
| Build and install | [docs/getting-started.md](/Users/otonashi/thinking/building/gaal/docs/getting-started.md) | First time setting up gaal, or troubleshooting build/install issues |

## Paths

| What | Path |
|------|------|
| Binary | `gaal` (symlink to `target/release/gaal`) |
| Data root | `~/.gaal/` (override with `GAAL_HOME` env var) |
| SQLite index | `$GAAL_HOME/index.db` |
| Tantivy FTS | `$GAAL_HOME/tantivy/` |
| Config | `$GAAL_HOME/config.toml` |
| Source repo | `/Users/otonashi/thinking/building/gaal/` |

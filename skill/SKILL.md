---
name: gaal
description: |
  Agent session observability CLI — turns AI coding sessions into first-class queryable artifacts.
  Parses Codex, Claude Code, Antigravity CLI (agy), Hermes, and Gemini traces, indexes into
  SQLite + Tantivy FTS, and renders token-efficient markdown views over raw evidence.
  Use when recalling prior context, attributing file/command activity, searching session history,
  inspecting sessions, rendering transcripts/activity, identifying the current session,
  generating optional continuity handoffs, or maintaining the local index.
  JSON default, -H for human. Gaal-shaped errors teach agents what went wrong + how to recover.
---

# gaal

Session observability for AI coding agents.

Codex, Claude Code, Antigravity CLI, Hermes, and Gemini leave traces on disk:
JSONL, SQLite, JSON, and harness-owned artifacts. Raw, they're usually the
wrong interface for the next agent. Gaal keeps the raw evidence in place,
indexes the boring facts, and renders smaller markdown views when an agent needs
to read.

The core idea: **sessions are first-class queryable artifacts, not throwaway logs.**

Every indexed session becomes searchable, attributable, and readable. Use Gaal
to resume work, attribute changes, search across sessions, and maintain
continuity across context resets without pretending that one summary is memory.

## Design Principles

**1. Two-source architecture.** Database for speed, source artifacts for truth.
SQLite powers `ls`, `inspect`, `who`, `search`, `resolve`, tags, handoff
metadata, and most structured queries. Raw traces on disk power transcript
rendering, activity slices, salt scanning, and source-level inspection. The DB
is derived from the files, not the other way around.

**2. Errors teach.** Gaal-shaped runtime errors have three parts: what went
wrong, a working example, and a hint for the next action. This is the AX
principle — agent experience matters more than API surface. A failed command
should be productive: parse the error, extract the example, retry. Plain Clap
parser errors can still happen before Gaal's formatter runs.

**3. Agent-native.** JSON by default. Exit codes are stable contracts (0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error). Composable with jq, pipeable into scripts, assertable with `jq -e`. `-H` is the human escape hatch, not the default.

**4. Taxonomy over heuristics.** Sessions have types — `standalone`, `coordinator`, `subagent` — not guessed states. We killed stuck detection, loop detection, velocity estimation, and process monitoring because heuristics lie more often than they help. What survived: deterministic classification based on observable structure.

**5. Evidence first.** When working on Gaal's own code: inspect real fixtures or
source traces before reasoning about schemas. When using Gaal: trust
`inspect --trace`, `transcript`, `activity`, and `resolve` output over guesses
about what a session contains. Ground truth is sacred — confident lies
compound.

## The Six Questions

Every gaal command answers exactly one question:

| Question | Command |
|----------|---------|
| What sessions exist? | `gaal ls` |
| What happened in this session? | `gaal inspect <id>` |
| Which sessions touched this? | `gaal who <verb> <target>` |
| Where does this text appear? | `gaal search <query>` |
| What past context is relevant? | `gaal recall <topic>` |
| How do I create continuity? | `gaal create-handoff <id>` |

Supporting commands: `transcript` (almost lossless markdown for one session),
`activity` (source-backed markdown across a time window), `resolve` (expand a
session reference to source and artifact paths), `salt`/`find-salt`
(self-identification), `index` (maintenance), and `tag` (annotation).

If you're unsure which command to reach for, match your intent to one of these six questions.

## Quick Start

```bash
gaal ls -H                              # fleet overview
gaal inspect latest --tokens -H         # drill into newest session
gaal who wrote AGENTS.md                # attribution
gaal search "auth refactor" --limit 5   # free-text search
gaal recall "auth refactor" --format brief
gaal transcript latest                  # rendered transcript path
gaal create-handoff latest --dry-run    # preview handoff generation
```

## First Launch

Do not guess setup. If Gaal is new on this machine or checkout, read:

- `skill/references/first-run.md`

Onboarding contract:

```bash
gaal onboard --dry-run
```

Use this after package install or binary update. It has no setup side effects:
it does not write skill files, index sessions, schedule jobs, install agent-mux,
or generate handoffs. It tells the installing agent where to load the latest
Gaal skill/reference material and which first-launch commands to run.

Minimal smoke path:

```bash
gaal --version
gaal index status
gaal ls -H --limit 5
```

If the binary or index is missing, follow the first-run reference. It owns
source install, scheduled indexing, handoff onboarding, first-use prompts, and
`GAAL_HOME` caveats.

## Session Taxonomy

```
standalone    — normal session, no subagents
coordinator   — parent that spawned subagents via Agent tool
subagent      — child spawned by a coordinator
```

`gaal ls` includes subagents by default. Use `--skip-subagents` for top-level
work only, or `--session-type subagent` / `--subagent-type <type>` to focus on
child sessions. `inspect` shows subagent relationships when the source traces
expose them. Attribution via `who` flows through the chain when parent-child
evidence exists.

## Continuity Protocol

**Start of session** — pull relevant context:
```bash
gaal recall "topic" --format brief --limit 5
```

**End of session** — preview a handoff only when continuity is useful:
```bash
gaal create-handoff <id> --dry-run
```

Recall searches generated handoffs, not raw traces. If recall returns nothing
useful, check `gaal index status`, then use `gaal search` or `gaal who` over raw
indexed facts before generating new handoffs.

## Self-Identification

When an agent needs to identify its own running session:

```bash
gaal salt
gaal find-salt GAAL_SALT_<hex>
```

`salt` and `find-salt` must be separate tool calls — the token has to flush into
the session log before scanning. Pass the literal token printed by `gaal salt`;
do not rely on shell variables persisting across agent tool calls.

`find-salt` scans Claude Code, Codex, Antigravity JSONL traces, and Grok visible
session sources. It does not identify Gemini JSON sessions or Hermes SQLite
sessions. For Grok, the returned `jsonl_path` is the session directory because
native Grok sessions are multi-file artifacts.

If a handoff is needed after self-identification, inspect the returned JSONL
path and run a dry-run first:

```bash
gaal create-handoff --jsonl /path/to/session.jsonl --dry-run
```

## Essential Patterns

**Quoted targets:** `who` accepts a free-form target. Put flags before long
multi-word targets, or quote the target exactly:
```bash
gaal who wrote --since 7d "src/main.rs"
gaal who ran --since 7d "cargo test"
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
- Live JSONL tailing (not a daemon, not a watcher)

These were deliberately removed, not deferred.

## Hard Rules

- **Read-only by default.** Most Gaal commands are pure queries. Commands that
  mutate local derived state: `index backfill`, `index reindex`, `index prune`,
  `index recover-orphans`, and `tag`. `create-handoff` is externally sensitive.
- **Handoffs are opt-in compression.** `create-handoff` can send transcript
  content to an external LLM backend and spend quota or credits. Always use
  `--dry-run` first; only run the matching non-dry-run command after the plan is
  acceptable and the task explicitly calls for a generated continuity artifact.
- **When developing Gaal itself:** read `AGENTS.md`, verify parser claims
  against fixtures or real traces, and run the relevant Rust/tests gate. Do not
  infer upstream trace schemas from memory.
- **Evidence first.** Grep real source shapes before reasoning about schemas.
  Confident guesses about trace structure are the #1 source of Gaal bugs.
- **Trust Gaal's normalized output, not ad hoc raw parsing.** Gaal normalizes
  multiple incompatible engine formats into one query surface.

## Anti-Patterns

| Do NOT | Do Instead | Why |
|--------|------------|-----|
| Assume `gaal ls` has `--status active` | Use `gaal ls --all` + `gaal inspect <id>` | Active monitoring was killed — heuristics lie. Taxonomy is deterministic, status was not |
| Read raw session JSONL manually | Use `gaal inspect --trace` or `gaal transcript` | Gaal normalizes dual formats; raw parsing will silently break on the other engine's schema |
| Call `gaal inspect` in a loop | Use `gaal inspect --ids a1b2,c3d4` | Batch mode exists — saves N-1 DB opens and N-1 process spawns |
| Assume `gaal recall` works without handoffs | Check `gaal index status` first | Recall queries the handoff index, not raw sessions. No handoffs = no recall results |
| Run handoff batches cold | Start with `gaal create-handoff --batch --dry-run` | Batch generation can spend quota quickly |
| Add `--engine` everywhere | Use it only where the command supports it | `create-handoff --engine` selects extraction engine, not source-session filtering |

## Reference

Full command flags, output schemas, format comparison tables, and operational details:

| Need | Where | Read when |
|------|-------|-----------|
| First run | `skill/references/first-run.md` | First setup, missing binary, missing index, scheduled indexing, handoff onboarding, first useful prompts |
| Verb reference | `skill/references/verb-reference.md` | Exact flags, supported engines, and command-specific caveats |
| Troubleshooting | `skill/references/troubleshooting.md` | No index, no results, ambiguous IDs, handoff provider issues |
| Command reference by group | `docs/commands/` | Public command docs and output examples |
| Architecture deep-dive | `docs/architecture.md` | Building on Gaal internals or extending parsers |

## Paths

| What | Path |
|------|------|
| Binary | `gaal` on `PATH`; source install usually places it under Cargo's bin directory, and scheduled jobs render the verified absolute path |
| Data root | `~/.gaal/` (override with `GAAL_HOME` env var) |
| SQLite index | `$GAAL_HOME/index.db` |
| Tantivy FTS | `$GAAL_HOME/tantivy/` |
| Config | `$GAAL_HOME/config.toml` |

---
name: gaal
description: |
  Gaal is the memory layer for AI coding agents on this Mac mini. It indexes Claude Code, Codex, Gemini, and Hermes sessions so agents can recall context, search traces, attribute file/command activity, identify their own running session, inspect transcripts, and create continuity handoffs. Use for prior session context, attribution, transcript retrieval, self-identification, handoff generation, fleet/session search, and gaal maintenance.
---

# gaal

strace for AI agents. Dissect anything that happened at the agentic level — every session, every tool call, every file touch, every command run. Claude Code, Codex, Gemini, and Hermes traces indexed and queryable.

## Capabilities

```bash
gaal ls -H                                      # fleet overview
gaal ls --engine hermes --since 7d -H           # filter sessions by engine
gaal inspect latest --tokens -H                 # inspect one session
gaal inspect latest --files write               # file writes only
gaal transcript latest                          # rendered transcript path metadata
gaal transcript latest --stdout                 # full transcript markdown
gaal activity --since 1d -H                     # source-backed activity slices
gaal who wrote skill/SKILL.md --since 7d        # attribution
gaal who ran cargo --failed                     # failed command attribution
gaal search "handoff provider" --limit 5        # full-text fact search
gaal recall "gaal handoff rewrite" --limit 5    # continuity context
gaal recall --id latest --format brief          # one session's handoff
gaal salt                                      # self-ID step 1
gaal find-salt GAAL_SALT_<hex>                  # self-ID step 2
gaal create-handoff latest --dry-run            # preview handoff generation
gaal resolve dc5e98dc -H                        # expand short ID to paths
gaal tag dc5e98dc research                      # annotate a session
gaal index status -H                            # index health
```

**Output format:** JSON by default (agent-native, pipeable to jq). Add `-H` for human-readable tables.

**Canonical binary on this Mac:** source lives at `/Users/otonashi/thinking/building/gaal`; the stable CLI is `/opt/homebrew/bin/gaal`, symlinked to `/Users/otonashi/thinking/building/gaal/target/release/gaal`. `~/.cargo/bin/gaal` is compatibility only and must point at the same release artifact.

**When in doubt:** `gaal --help` is excellent documentation. Works per-verb too: `gaal who --help`, `gaal create-handoff --help`, etc.

## Fleet

`gaal ls` lists indexed sessions. Important filters: `--engine claude|codex|gemini|hermes`, `--session-type coordinator|standalone|subagent`, `--subagent-type <type>`, `--since`, `--before`, `--cwd`, repeatable `--tag`, `--sort started|ended|tokens|cost|duration`, `--aggregate`, `--all`, and `--skip-subagents`.

Session taxonomy:

```text
standalone   normal session
coordinator  parent that spawned subagents
subagent     child spawned by a coordinator
```

Subagents are included by default. Use `--skip-subagents` for top-level work only.

## Inspect

Use `inspect` for structured details and focused views:

```bash
gaal inspect latest
gaal inspect 249aad1e --trace
gaal inspect latest --files read
gaal inspect latest --commands
gaal inspect --ids a1b2c3d4,e5f6g7h8 --files write
```

Use `transcript` for rendered markdown:

```bash
gaal transcript latest                 # JSON path, size, token estimate
gaal transcript latest --stdout        # markdown content
gaal transcript latest --force         # re-render cached transcript
```

Prefer `inspect --trace` or `transcript` over raw file parsing. Gaal normalizes incompatible Claude Code JSONL, Codex JSONL, Gemini JSON, and Hermes SQLite formats.

## Activity

Use `gaal activity` for source-backed historical slices across sessions. It renders transcript-shaped markdown for `[since,before)` windows, including long-running sessions that started earlier; it is not live monitoring.

Examples: `gaal activity --since 1d -H`, `gaal activity --since 2026-05-25 --before 2026-05-26 --stdout`.

## Attribution

`gaal who <verb> <target>` finds sessions that performed an action. Verbs are `read`, `wrote`, `ran`, `touched`, `changed`, and `deleted`.

Examples: `gaal who read src/main.rs --since 30d`, `gaal who wrote skill/SKILL.md --cwd /Users/otonashi/thinking/building/gaal`, and `gaal who ran cargo --failed --limit 20`.

Attribution flows through subagent chains — if a subagent wrote a file, gaal traces it back through the parent coordinator.

## Search And Recall

`gaal search` searches indexed facts. Fields are `prompts`, `replies`, `commands`, `errors`, `files`, or `all`.

```bash
gaal search "openrouter" --field all --limit 10
gaal search "cargo test" --field commands --since 14d
```

`gaal recall` retrieves continuity context from indexed handoffs:

```bash
gaal recall "auth migration" --days-back 30 --limit 5 --format brief
gaal recall --id abc12345 --format handoff
```

`recall` requires either a query or `--id`; no arguments prints help and exits 0. Formats are `summary`, `brief`, `handoff`, `full`, and `eywa`. Backfill indexes sessions and facts, not handoffs. If recall is empty, check `gaal index status` for handoff counts and generate handoffs with `gaal create-handoff` only when needed.

## Self-ID

When an agent needs to discover its OWN session ID (e.g., to create its own handoff), use the two-step salt protocol:

```bash
gaal salt                          # step 1: prints GAAL_SALT_<hex>
gaal find-salt GAAL_SALT_<hex>     # step 2: returns session metadata
```

Two separate tool calls required — the salt must flush to the session log before `find-salt` can scan for it. Pass the literal token; shell variables don't persist across agent tool calls. `find-salt` scans Claude Code and Codex JSONL only. Gemini salt scanning is not implemented.

**Fallback:** If salt scanning fails (sandbox, unflushed logs): `gaal inspect latest --source` returns the most recent session's JSONL path.

**Chain to handoff:** `gaal find-salt GAAL_SALT_<hex> | jq -r .jsonl_path | xargs gaal create-handoff --jsonl`

When indexed, `find-salt` returns model, cwd, session type, turns, tokens, transcript status, and handoff status. When not indexed, it returns session ID, engine, JSONL path, and `"indexed": false`.

## Handoffs

`gaal create-handoff` generates continuity markdown via an external LLM backend and can cost money.

**99% use case:** `gaal create-handoff <session-id>` — pass the 8-character short ID.

**Session ID formats by engine:**
- **Claude Code**: first 8 chars of UUID (e.g., `69384ef1`)
- **Gemini**: first 8 chars of session name
- **Codex**: last 8 hex chars of UUIDv7 (dashes stripped) — Codex UUIDs share timestamp prefix, uniqueness is in the suffix
- **Hermes**: full session ID. First-8 prefixes are date-like and collide.

```bash
gaal create-handoff 69384ef1                              # 99% case
gaal create-handoff latest --dry-run                      # preview
gaal create-handoff latest --provider openrouter          # alt backend
gaal create-handoff --batch --since 1d --min-turns 3 --dry-run
```

Key flags: `--engine claude|codex|gemini|hermes`, `--provider agent-mux|openrouter`, `--effort low|medium|high|xhigh`, `--dry-run`, `--batch`, `--parallel`, `--min-turns`, `--this`, `--jsonl`, `--model`, and `--prompt`. Use `--dry-run` before batch work.

**Failure mode:** Default provider is `agent-mux`. If agent-mux is unavailable, `create-handoff` hangs. Verify agent-mux is working before batch operations.

## Engine Filters

`--engine claude|codex|gemini|hermes` is supported only on:

```text
gaal ls
gaal who
gaal search
gaal activity
gaal resolve
gaal create-handoff
gaal index backfill
```

Do not use `--engine` with `inspect`, `recall`, `transcript`, `find-salt`, or `tag`.

## Index And Tags

Index commands maintain derived data:

```bash
gaal index status
gaal index backfill --engine claude
gaal index backfill --engine hermes
gaal index reindex <session-id>
gaal index recover-orphans --dry-run
```

`backfill` is incremental and indexes discovered sessions into the database; it does not create handoffs. `recover-orphans` is a special repair command for orphaned Claude subagent files; run `--dry-run` first.

**Stuck cursor recovery:** If backfill gets wedged, reset mtime gates: `sqlite3 ~/.gaal/index.db "DELETE FROM meta WHERE key LIKE 'backfill:%'"` then re-run backfill.

Tag with `gaal tag <session-id> research`, remove with `gaal tag <session-id> --remove research`, and list tags with `gaal tag ls`.

## Hard Rules

- Read-only commands should be preferred. Mutating commands are `create-handoff`, `index backfill`, `index reindex`, `index prune`, `index import-eywa`, `index recover-orphans`, and `tag`.
- JSON is the agent default. Stable exit codes: `0` success, `1` no results, `2` ambiguous ID, `3` not found, `10` no index, `11` parse error.
- The database is derived from local session files. When developing Gaal itself, verify parser claims against real traces and run `cargo build --release`; `/opt/homebrew/bin/gaal` and `~/.cargo/bin/gaal` must both resolve to the release build.
- Services and scheduled scripts should prefer `/opt/homebrew/bin/gaal` or put `/opt/homebrew/bin` before cargo paths to avoid duplicate-binary drift.

## Anti-Patterns

| Do NOT | Do Instead |
|--------|------------|
| Add `--engine` everywhere | Use it only on commands listed in Engine Filters |
| Run `gaal recall` with no query expecting results | Provide a query or `--id` |
| Treat `index backfill` as handoff generation | Use `create-handoff` for handoffs |
| Assume `find-salt` works for Gemini | Use it for Claude Code/Codex only |
| Read raw session files for normal work | Use `inspect`, `transcript`, `who`, `search`, or `recall` |
| Run paid handoff batches cold | Start with `create-handoff --batch --dry-run` |
| Use debug builds when changing Gaal | Run `cargo build --release` |
| Treat activity as live process status | It is source-backed history; use fleet/process tools for live status |

## Reference

For exact flags, schemas, and operational details, read `docs/agent-guide.md`, `docs/commands/`, `docs/formats.md`, `docs/architecture.md`, `docs/getting-started.md`, and the skill-local files in `skill/references/`.

**Data root:** `~/.gaal/` (override with `GAAL_HOME` env var). Index at `$GAAL_HOME/index.db`, FTS at `$GAAL_HOME/tantivy/`. Hermes discovery reads `~/.hermes/state.db` by default; override with `HERMES_STATE_DB` or `HERMES_HOME`.

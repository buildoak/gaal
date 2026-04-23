---
name: gaal
description: |
  Gaal is the memory layer for AI coding agents on this Mac mini. It indexes Claude Code, Codex, and Gemini sessions so agents can recall context, search traces, attribute file/command activity, identify their own running session, inspect transcripts, and create continuity handoffs. Use for prior session context, attribution, transcript retrieval, self-identification, handoff generation, fleet/session search, and gaal maintenance.
---

# gaal

Gaal turns local Claude Code, Codex, and Gemini traces into queryable agent memory.

## Capabilities

```bash
gaal ls -H                                      # fleet overview
gaal ls --engine gemini --since 7d -H           # filter sessions by engine
gaal inspect latest --tokens -H                 # inspect one session
gaal inspect latest --files write               # file writes only
gaal transcript latest                          # rendered transcript path metadata
gaal transcript latest --stdout                 # full transcript markdown
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

Default output is JSON; use `-H` for human-readable output.

## Fleet

`gaal ls` lists indexed sessions. Important filters: `--engine claude|codex|gemini`, `--session-type coordinator|standalone|subagent`, `--subagent-type <type>`, `--since`, `--before`, `--cwd`, repeatable `--tag`, `--sort started|ended|tokens|cost|duration`, `--aggregate`, `--all`, and `--skip-subagents`.

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

Prefer `inspect --trace` or `transcript` over raw file parsing. Gaal normalizes incompatible Claude Code JSONL, Codex JSONL, and Gemini JSON formats.

## Attribution

`gaal who <verb> <target>` finds sessions that performed an action. Verbs are `read`, `wrote`, `ran`, `touched`, `changed`, and `deleted`.

Examples: `gaal who read src/main.rs --since 30d`, `gaal who wrote skill/SKILL.md --cwd /Users/otonashi/thinking/building/gaal`, and `gaal who ran cargo --failed --limit 20`.

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

Self-identification is two separate shell/tool calls so the salt output is flushed into the session log:

```bash
gaal salt
gaal find-salt GAAL_SALT_<hex>
```

Pass the literal token from `gaal salt`; do not rely on shell variables across agent tool calls. `find-salt` scans Claude Code and Codex JSONL only (`~/.claude/projects` and `~/.codex`). Gemini salt scanning is not implemented.

When indexed, `find-salt` returns model, cwd, session type, turns, tokens, transcript status, and handoff status. When not indexed, it returns session ID, engine, JSONL path, and `"indexed": false`.

## Handoffs

`gaal create-handoff` generates continuity markdown via an external LLM backend and can cost money.

```bash
gaal create-handoff latest --dry-run
gaal create-handoff latest --provider openrouter
gaal create-handoff --batch --since 1d --min-turns 3 --dry-run
```

Key flags: `--engine claude|codex|gemini`, `--provider agent-mux|openrouter`, `--effort low|medium|high|xhigh`, `--dry-run`, `--batch`, `--parallel`, `--min-turns`, `--this`, `--jsonl`, `--model`, and `--prompt`. Use `--dry-run` before batch work.

## Engine Filters

`--engine claude|codex|gemini` is supported only on:

```text
gaal ls
gaal who
gaal search
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
gaal index reindex <session-id>
gaal index recover-orphans --dry-run
```

`backfill` is incremental and indexes discovered sessions into the database; it does not create handoffs. `recover-orphans` is a special repair command for orphaned Claude subagent files; run `--dry-run` first.

Tag with `gaal tag <session-id> research`, remove with `gaal tag <session-id> --remove research`, and list tags with `gaal tag ls`.

## Hard Rules

- Read-only commands should be preferred. Mutating commands are `create-handoff`, `index backfill`, `index reindex`, `index prune`, `index import-eywa`, `index recover-orphans`, and `tag`.
- JSON is the agent default. Stable exit codes: `0` success, `1` no results, `2` ambiguous ID, `3` not found, `10` no index, `11` parse error.
- The database is derived from local session files. When developing Gaal itself, verify parser claims against real traces and run `cargo build --release`; the installed binary points at the release build.

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

## Reference

For exact flags, schemas, and operational details, read `docs/agent-guide.md`, `docs/commands/`, `docs/formats.md`, `docs/architecture.md`, `docs/getting-started.md`, and the skill-local files in `skill/references/`.

# Agent Guide

`gaal` is built for AI agents first. Treat it as a query surface over session history, not as raw-log storage. The default interface is JSON so agents can branch on fields, assert invariants with `jq`, and compose `gaal` into pipelines without scraping human tables.

This page is the complete guide for a cold agent to become productive with `gaal`. If you read only one page, read this one.

## Decision Tree

Use this table first. It is the fastest way to choose the correct command.

| Need | Command |
|------|---------|
| Fleet overview / recent sessions | `gaal ls` |
| Drill into one session | `gaal inspect <id>` |
| Get rendered transcript markdown | `gaal transcript <id>` |
| Render activity across a time window | `gaal activity --since <window>` |
| Fleet totals | `gaal ls --aggregate` |
| Who wrote/read/ran X? | `gaal who <verb> <target>` |
| Free-text search across content | `gaal search <query>` |
| Handoff recall for continuity | `gaal recall [query]` |
| Generate handoff document | `gaal create-handoff <id>` |
| Self-identify current session | `gaal salt` -> later `gaal find-salt` -> optional `gaal create-handoff --jsonl ... --dry-run` |
| Cross-session prompt injection | `session-ctl` (different tool) |

## Overview

The primary consumers of `gaal` are agents, not humans. Prefer machine-readable JSON unless a human explicitly asks for a table or card view.

`gaal` supports five engines: Claude Code (JSONL), Codex (JSONL), Gemini (single JSON), Antigravity CLI / `agy` (JSONL), and Hermes Agent (SQLite). All five are indexed into the same session, fact, transcript, tag, and handoff model. Agy support is native, experimental, and does not require agent-mux. Hermes support is newer and has been tested against one real installation shape plus sanitized fixtures; treat unusual Hermes installations as compatibility work until fixtures cover them.

The core mental model:

- `gaal ls` answers "what sessions exist?"
- `gaal inspect` answers "what happened in this session?"
- `gaal transcript` answers "give me the rendered markdown artifact"
- `gaal activity` answers "what happened across sessions in this time window?"
- `gaal who` answers "which sessions touched this thing?"
- `gaal search` answers "where does this text appear?"
- `gaal recall` answers "what past handoffs are relevant to this work?"
- `gaal create-handoff` answers "generate continuity material for future agents"

Before depending on `recall`, make sure handoffs and the index actually exist.

## Output Contract

- Default output is JSON for normal query commands.
- Use `-H` for human-readable tables or cards.
- `gaal salt` is an intentional exception: it prints a raw token string.
- JSON errors include `hint` and `example` fields alongside `ok`, `error`, and `exit_code` after CLI parsing succeeds. Argument parsing errors come from the CLI parser and may be plain text.
- Exit codes are stable:

| Exit code | Meaning |
|-----------|---------|
| `0` | success |
| `1` | no results |
| `2` | ambiguous ID |
| `3` | not found |
| `10` | no index |
| `11` | parse error |

This means agents should branch on both process exit status and JSON payload shape. A command can fail usefully and still tell you the exact next command to run.

## Session ID Resolution

Commands that take a session identifier accept several forms:

- Full UUID: accepted even though `gaal` truncates internally
- Short ID: the 8-character session identifier used directly. For Hermes this
  is the persisted alias, not the first 8 characters of the full native ID.
- Prefix: any unique prefix resolves; non-unique prefixes return ambiguous-ID error with exit code `2`
- `latest`: resolves to the most recent session
- `today`: accepted by `gaal create-handoff` for the current day's sessions

Smallest defensible rule: use short IDs when you have them, use the registered
Hermes alias for Hermes sessions, and use `latest` when you do not care which
exact recent session is selected.

## Common Patterns

### Recall at session start

Use `recall` when you are resuming work and want continuity, not when you need raw session facts. Use `recall --id <session-id>` when you know which session's handoff you want. Use `recall <query>` when searching by topic.

```bash
gaal recall 'topic' --format brief --limit 5
gaal recall --id abc12345 --format brief -H
gaal recall --id latest --format handoff
```

### Handoff at session end

Use `create-handoff` when wrapping up a session or producing a continuity artifact for another agent.

```bash
gaal create-handoff latest --dry-run
gaal create-handoff latest                  # only after reviewing the dry run
gaal create-handoff latest --effort high --dry-run
gaal create-handoff --batch --since 1d --dry-run
```

### Self-handoff protocol

Use this when the agent must identify its own current session and generate a handoff from that exact JSONL. `find-salt` returns enriched session context (model, type, tokens, transcript path, handoff status) when the session is indexed, so you get full self-identification in one call.

First tool call:

```bash
gaal salt
```

Second, later tool call after the salt has been written into the session log:

```bash
gaal find-salt GAAL_SALT_<hex>
```

`find-salt` returns full session context when indexed: model, type, tokens,
transcript path, JSONL path, and handoff status. Inspect that JSON before
generating anything.

`find-salt` scans Claude Code, Codex, and Agy/Antigravity transcript logs. It
does not identify Gemini or Hermes sessions.

If a handoff is needed, preview first:

```bash
gaal create-handoff --jsonl /path/to/session.jsonl --dry-run
gaal create-handoff --jsonl /path/to/session.jsonl
```

CRITICAL: `gaal salt` and `gaal find-salt` must be separate tool calls. The JSONL must flush between those calls or `find-salt` may miss the current session. Do not hide this split inside one shell script.

### Filtering by engine

Use `--engine` to narrow any fleet or attribution query to a single engine.

```bash
# Gemini sessions only
gaal ls --engine gemini --since 7d -H

# Who wrote a file, Gemini sessions only
gaal who wrote CLAUDE.md --engine gemini

# Search within Gemini sessions
gaal search "parser" --engine gemini
```

Agy sessions use `--engine agy`. Their source store is
`~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`,
with fallback to `transcript.jsonl`; the indexed ID is the first 8 characters of
the brain UUID.

### Finding subagent dispatches

Use `--subagent-type` to filter sessions by the type of Agent tool dispatch.

```bash
# All worker dispatches in the last 7 days
gaal ls --subagent-type worker --since 7d -H

# Explore-type dispatches
gaal ls --subagent-type Explore --since 7d -H

# Or use tags (auto-applied on indexing)
gaal ls --tag worker -H
```

The `subagent_type` field is extracted from the Agent `tool_use` input in parent JSONL and populated during `gaal index backfill`. Run `gaal index backfill --force` after upgrading to populate existing sessions.

### Composable pipelines

Use JSON output as the default transport layer.

```bash
gaal ls --since today | jq -r '.sessions[].id' | xargs -I{} gaal inspect {} --files write
```

### jq assertion pattern

Use `jq -e` to turn CLI output into an assertion gate.

```bash
gaal ls --limit 1 | jq -e '.sessions | length == 1 and all(.[]; .id and .engine)' > /dev/null
```

## Pipe Gotchas

The `who` verb consumes trailing arguments greedily. Do not pipe `gaal who ...` directly into another command if the shell could alter argument grouping. Capture it first, then pipe the captured JSON.

```bash
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.'
```

Transcript behavior is also easy to misuse:

- `gaal transcript <id>` returns path metadata by default
- Use `--stdout` only when you explicitly want the markdown content in the current calling context

If you want a file path for later consumption, do not add `--stdout`.

## Anti-Patterns

Avoid these patterns. They usually create incorrect assumptions or unnecessary work.

| Do NOT | Do instead |
|--------|------------|
| Pipe `gaal who` directly | Capture to a variable first |
| Assume `gaal ls` has `--status active` | Use `gaal ls --all` plus `gaal inspect <id>` |
| Read entire session JSONL manually | Use `gaal inspect --trace` or `gaal transcript` |
| Treat `gaal activity` as live process status | Use it for historical/indexed activity; use fleet/process tools for live status |
| Call `gaal inspect` in a loop | Use `gaal inspect --ids a1b2,c3d4` |
| Assume `gaal recall` works without handoffs | Check gaal index status first |
| Run default `gaal create-handoff` without a configured handoff backend | Verify `agent-mux` availability first, or choose and test another supported provider |

## Sandbox Usage

By default `gaal` stores its database, Tantivy index, and config under `~/.gaal/`. In sandboxed environments (Codex workers, CI containers) that path is often read-only or remapped. Set `GAAL_HOME` to relocate the data directory:

```bash
# Point gaal at a writable location
export GAAL_HOME=/tmp/gaal-workspace
gaal ls

# One-liner for a single command
GAAL_HOME=/tmp/gaal-workspace gaal inspect latest -H
```

The resolution order is:
1. `GAAL_HOME` environment variable (if set and non-empty)
2. `~/.gaal/` (default)

When dispatching workers that need gaal access from a sandboxed harness, export `GAAL_HOME` before the dispatch so child processes inherit it.

## Comparing Sessions

```bash
# Compare two sessions side by side
gaal inspect --ids a1b2,c3d4 --tokens -H

# Compare all sessions from a time range
gaal ls --since 3d --aggregate -H

# Get token totals for a specific project
gaal ls --cwd /path/to/project --aggregate -H
```

## Security And Approval Notes

`gaal` is read-only by default, but not every command is harmless.

- Read-only commands are the safe default for agents
- Mutation commands include `create-handoff`, `index backfill`, `index reindex`, `index prune`, `index recover-orphans`, and `tag`
- `create-handoff` dispatches to an LLM/agent backend and may consume subscription quota, API credits, metered usage, or local compute. `agent-mux` is the default backend for handoff generation, but core indexing, search, inspect, attribution, transcript, and tag workflows do not require it.
- Use `--dry-run` before batch handoff generation
- `index backfill` is operationally safe: it reads JSONL and writes derived state under `~/.gaal/`

Practical agent rule: do not mutate anything unless the task explicitly requires continuity generation, tagging, or index maintenance.

## AX Error Handling

Every `gaal` error is designed to teach the next action. A useful error has three parts:

1. What went wrong: specific and actionable
2. A working example: a valid invocation the agent can copy
3. A hint: the next command to try

Example:

```text
$ gaal inspect nonexistent -H
What went wrong: Session nonexistent was not found.
Example: gaal inspect latest -H
Hint: List recent sessions with gaal ls --since 7d -H
```

For agents, this means failed commands are often still productive. Parse the error, extract the example or hint, and retry with the suggested valid form.

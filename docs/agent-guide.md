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
| Fleet totals | `gaal ls --aggregate` |
| Who wrote/read/ran X? | `gaal who <verb> <target>` |
| Free-text search across content | `gaal search <query>` |
| Semantic recall for continuity | `gaal recall [query]` |
| Generate handoff document | `gaal create-handoff <id>` |
| Self-identify current session | `gaal salt` -> `gaal find-salt` -> `gaal create-handoff --jsonl` |
| Cross-session prompt injection | `session-ctl` (different tool) |

## Overview

The primary consumers of `gaal` are agents, not humans. Prefer machine-readable JSON unless a human explicitly asks for a table or card view.

The core mental model:

- `gaal ls` answers "what sessions exist?"
- `gaal inspect` answers "what happened in this session?"
- `gaal transcript` answers "give me the rendered markdown artifact"
- `gaal who` answers "which sessions touched this thing?"
- `gaal search` answers "where does this text appear?"
- `gaal recall` answers "what past handoffs are relevant to this work?"
- `gaal create-handoff` answers "generate continuity material for future agents"

Before depending on `recall`, make sure handoffs and the index actually exist.

## Output Contract

- Default output is JSON.
- Use `-H` for human-readable tables or cards.
- JSON errors include `hint` and `example` fields alongside `ok`, `error`, and `exit_code`.
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
- Short ID: the 8-character session prefix, used directly
- Prefix: any unique prefix resolves; non-unique prefixes return ambiguous-ID error with exit code `2`
- `latest`: resolves to the most recent session
- `today`: accepted by `gaal create-handoff` for the current day's sessions

Smallest defensible rule: use short IDs when you have them, and use `latest` when you do not care which exact recent session is selected.

## Common Patterns

### Recall at session start

Use `recall` when you are resuming work and want continuity, not when you need raw session facts. Use `recall --id <session-id>` when you know which session's handoff you want. Use `recall <query>` when searching by topic.

```bash
gaal recall --format eywa
gaal recall 'topic' --format eywa --limit 5
gaal recall --id abc12345 --format brief -H
gaal recall --id latest --format handoff
```

### Handoff at session end

Use `create-handoff` when wrapping up a session or producing a continuity artifact for another agent.

```bash
gaal create-handoff
gaal create-handoff --effort high           # override effort level (low/medium/high/xhigh)
gaal create-handoff --batch --since 1d --dry-run
```

### Self-handoff protocol

Use this when the agent must identify its own current session and generate a handoff from that exact JSONL. `find-salt` returns enriched session context (model, type, tokens, transcript path, handoff status) when the session is indexed, so you get full self-identification in one call.

```bash
SALT=$(gaal salt)
echo "$SALT"
# find-salt returns full session context: model, type, tokens, transcript, handoff status
RESULT=$(gaal find-salt "$SALT")
JSONL=$(echo "$RESULT" | jq -r .jsonl_path)
# Check if handoff already exists before generating
HAS_HANDOFF=$(echo "$RESULT" | jq -r .handoff.exists)
if [ "$HAS_HANDOFF" != "true" ]; then
  gaal create-handoff --jsonl "$JSONL"
fi
```

CRITICAL: `gaal salt` and `gaal find-salt` must be separate tool calls. The JSONL must flush between those calls or `find-salt` may miss the current session.

### Finding GSD dispatches

Use `--subagent-type` to filter sessions by the type of Agent tool dispatch.

```bash
# All GSD-Heavy dispatches in the last 7 days
gaal ls --subagent-type gsd-heavy --since 7d -H

# All GSD-coordinator (legacy GSD-Heavy type name) dispatches
gaal ls --subagent-type gsd-coordinator --since 7d -H

# Explore-type dispatches
gaal ls --subagent-type Explore --since 7d -H

# Or use tags (auto-applied on indexing)
gaal ls --tag gsd-heavy -H
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
| Call `gaal inspect` in a loop | Use `gaal inspect --ids a1b2,c3d4` |
| Assume `gaal recall` works without handoffs | Check gaal index status first |
| Run `gaal create-handoff` without `agent-mux` | Verify `agent-mux` availability first |

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
- Mutation commands include `create-handoff`, `index backfill`, `index reindex`, `index prune`, `index import-eywa`, and `tag`
- `create-handoff` dispatches to an LLM through `agent-mux` or OpenRouter, so it can cost money
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

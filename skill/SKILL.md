---
name: gaal
description: |
  Agent session observability CLI for Claude Code and Codex. Query sessions, facts,
  transcripts, tags, handoffs, and subagents. JSON output by default, `-H` for human
  output. AX errors teach agents what went wrong and how to recover.
  Use when: session observability, session history, who wrote/read/ran a file,
  search sessions, fleet view, inspect session details, handoff generation, recall
  context from past sessions, transcript rendering, tag management, self-identification,
  salt token, subagents, token accounting, cache tokens, continuity, prior context.
  Replaces eywa for session recall and handoff generation.
  Do NOT use for: cross-session prompt injection (use session-ctl), live JSONL tailing
  (not supported), real-time process monitoring (removed in v0.1.0).
---

# gaal

Agent session observability CLI. 11 commands. JSON default, `-H` for human-readable output.
See [`DOCS.md`](/Users/otonashi/thinking/building/gaal/DOCS.md) for the canonical user doc and exact flag/output examples.

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
| Canonical docs | [`DOCS.md`](/Users/otonashi/thinking/building/gaal/DOCS.md) |

## Config Defaults (`~/.gaal/config.toml`)

| Key | Default | What |
|-----|---------|------|
| `llm.default_engine` | `codex` | Engine for handoff generation |
| `llm.default_model` | `gpt-5.3-codex-spark` | Model for handoff generation |
| `llm.timeout_secs` | `120` | LLM timeout |
| `handoff.prompt` | `prompts/handoff.md` | Extraction prompt path (relative to `~/.gaal/`) |
| `handoff.format` | `eywa` | Default handoff output format |
| `agent-mux.path` | `agent-mux` | agent-mux binary path |

## Eywa Migration Map

| Eywa command | Gaal equivalent |
|-------------|----------------|
| `eywa get` | `gaal recall --format eywa` |
| `eywa get "query"` | `gaal recall "query" --format eywa` |
| `eywa get "topic" --days-back 30 --max 5` | `gaal recall "topic" --days-back 30 --limit 5 --format eywa` |
| `eywa extract` | `gaal create-handoff` |
| `eywa extract <id>` | `gaal create-handoff <id>` |
| `eywa rebuild-index` | `gaal index backfill` |

The `--format eywa` flag produces coordinator-compatible output matching eywa's layout.

## Session Protocol

### Session Start

Use recall to pull context before starting work.

```bash
gaal recall --format eywa
gaal recall "sorbent operations" --format eywa --limit 5
gaal recall "gaussian moat" --days-back 30 --format eywa
```

`gaal recall` defaults to `--format brief`. Use `--format summary`, `--format handoff`, `--format full`, or `--format eywa` when you need a different shape.

### Session End

Write a short session summary first, then generate the handoff.

```bash
gaal create-handoff
gaal create-handoff <session-id>
gaal create-handoff --batch --since 1d --dry-run
gaal create-handoff --batch --since 1d
```

Handoff generation uses LLM via agent-mux by default. `--provider openrouter` is also available. Use `--dry-run` before batch runs.

### Self-Handoff Protocol

When the current session needs to identify its own JSONL file, use the salt flow.

```bash
SALT=$(gaal salt)
echo "$SALT"
JSONL=$(gaal find-salt "$SALT" | jq -r .jsonl_path)
gaal create-handoff --jsonl "$JSONL"
```

`gaal salt` and `gaal find-salt` must be separate tool calls so the salt is flushed to JSONL before scanning.

### Index Freshness

Recall quality depends on indexed handoffs. Use the index subcommands when freshness matters:

```bash
gaal index status
gaal index backfill
gaal index reindex <id>
gaal index import-eywa [PATH]
gaal index prune --before <DATE>
```

See [`DOCS.md`](/Users/otonashi/thinking/building/gaal/DOCS.md) for subcommand flags such as `--with-markdown`, `--output-dir`, and `--force`.

## Architecture Notes

- Two-source model: database-backed metadata and filesystem discovery of raw JSONL plus `subagents/agent-*.jsonl`.
- The DB is the fast query surface for `ls`, `inspect`, `who`, `recall`, tags, and most session metadata.
- The filesystem is still needed for transcript rendering, salt scanning, and subagent discovery.
- Parent JSONL `toolUseResult` blocks provide aggregate subagent metadata such as `agentId`, duration, total tokens, status, and prompt/description.
- Subagent JSONL files provide the full turn-by-turn trace and tool activity.
- This is why `inspect`, `transcript`, `who`, and `search` can show subagent-aware results while still surfacing the parent linkage.

## Quick Reference

```bash
gaal ls -H
gaal inspect latest --tokens -H
gaal who wrote CLAUDE.md
gaal search "gaussian moat" --limit 5
gaal transcript latest
gaal create-handoff latest
```

## Decision Tree

| Need | Tool |
|------|------|
| Fleet overview / recent sessions | `gaal ls` |
| Drill into one session | `gaal inspect <id>` |
| Get rendered transcript markdown for one session | `gaal transcript <id>` |
| Fleet totals instead of individual sessions | `gaal ls --aggregate` |
| Session health / operational snapshot | `gaal inspect <id>` |
| "Who wrote/read/ran X?" | `gaal who <verb> <target>` |
| Free-text search across content | `gaal search <query>` |
| Semantic recall | `gaal recall [query]` |
| Generate handoff document | `gaal create-handoff <id\|today>` |
| Self-identify current session | `gaal salt` -> `gaal find-salt <SALT>` -> `gaal create-handoff --jsonl` |
| Cross-session prompt injection | session-ctl (different tool) |

## Commands by Tier

### Fleet View

| Command | What |
|---------|------|
| `gaal ls` | List sessions. Filters: `--engine`, `--since`, `--before`, `--cwd`, `--tag`, `--sort`, `--limit`. Default limit is 10. |
| `gaal ls --aggregate` | Return aggregate totals instead of rows. |
| `gaal ls --all` | Include short/noise sessions. |
| `gaal ls --skip-subagents` | Hide subagent sessions and show only standalone/coordinator sessions. |

Human `ls` output uses a `Task` column for the session headline and shows subagent type badges when subagents are present.

### Drill-Down

| Command | What |
|---------|------|
| `gaal inspect <id>` | Session detail view. `latest` resolves the newest session. |
| `gaal inspect <id> --files write` | File ops view; bare `--files` defaults to `all`. |
| `gaal inspect <id> --errors` | Errors and non-zero exits only. |
| `gaal inspect <id> --commands` | Commands only. |
| `gaal inspect <id> --git` | Git operations only. |
| `gaal inspect <id> --tokens` | Token usage breakdown. |
| `gaal inspect <id> --trace` | Full event timeline. |
| `gaal inspect <id> --source` | Raw JSONL source path. |
| `gaal inspect <id> --include-empty` | Include empty/low-signal subagents in coordinator views. |
| `gaal inspect --ids a1b2,c3d4` | Batch IDs in comma-delimited form. |
| `gaal inspect --tag <tag>` | Batch filter by tag. |
| `gaal transcript <id>` | Transcript path metadata by default. |
| `gaal transcript <id> --stdout` | Dump rendered transcript markdown to stdout. |
| `gaal transcript <id> --force` | Re-render even if cached file exists. |

### Inverted Queries

| Command | What |
|---------|------|
| `gaal who read <path>` | Sessions that read a file. |
| `gaal who wrote <path>` | Sessions that modified a file. |
| `gaal who ran "<cmd>"` | Sessions that ran a command. |
| `gaal who touched <term>` | Broadest match across files and commands. |
| `gaal who changed <path>` | Sessions that changed a file. |
| `gaal who deleted <path>` | Sessions that deleted a file or removed it via command. |

`who` flags: `--since` default `7d`, `--before`, `--cwd`, `--engine`, `--tag`, `--failed` for `ran`, `--limit` default `10`, `-F/--full`, `-H/--human`.

### Search & Recall

| Command | What |
|---------|------|
| `gaal search "<query>"` | Full-text search over indexed facts. Flags: `--since` default `30d`, `--cwd`, `--engine`, `--field` default `all`, `--context` default `2`, `--limit` default `20`, `-H`. |
| `gaal recall [query]` | Semantic session retrieval. Flags: `--days-back` default `14`, `--limit` default `3`, `--format` default `brief`, `--substance` default `1`, `-H`. |

### LLM-Powered

| Command | What |
|---------|------|
| `gaal create-handoff <id\|today>` | Generate a handoff doc via LLM extraction. Flags: `--jsonl`, `--engine`, `--model`, `--prompt`, `--provider` default `agent-mux`, `--format` default `eywa-compatible`, `--batch`, `--since` default `7d`, `--parallel` default `1`, `--min-turns` default `3`, `--this`, `--dry-run`, `-H`. |

### Self-Identification

| Command | What |
|---------|------|
| `gaal salt` | Generate a unique salt token for self-identification. `-H` is supported; output is otherwise a raw token string. |
| `gaal find-salt <SALT>` | Find the first JSONL file containing the token. Returns `session_id`, `engine`, and `jsonl_path`. `-H` is supported. |

### Index Management

| Command | What |
|---------|------|
| `gaal index backfill` | Index all existing JSONL files. Flags: `--engine`, `--since`, `--force`, `--with-markdown`, `--output-dir`, `-H`. |
| `gaal index status` | Show index health/status. `-H` is supported. |
| `gaal index reindex <id>` | Force re-index of one session. `-H` is supported. |
| `gaal index import-eywa [PATH]` | Import legacy eywa handoff-index data. `-H` is supported. |
| `gaal index prune --before <DATE>` | Remove old facts before a date. `-H` is supported. |
| `gaal tag ls` | List all tags. This is the `gaal tag` listing form, not a separate subcommand. |
| `gaal tag <id> <tags...>` | Add tags to a session. |
| `gaal tag <id> <tags...> --remove` | Remove tags from a session. |

## Session ID Resolution

Gaal resolves short session IDs and prefixes where possible. Use `latest` for the newest session. Full UUIDs are truncated internally to the stored short ID.

What you can pass to gaal commands (`inspect`, `transcript`, `create-handoff`, `who`, `search`, and index/session workflows where applicable):

| Input | Behavior |
|-------|----------|
| Full UUID | Truncated internally to the stored short ID |
| Short ID | Used directly for lookup |
| Prefix | Resolves if unique, otherwise returns an ambiguous-ID error |
| `latest` | Resolves to the most recent session |

## Output Contract

- Default output is JSON.
- `-H` / `--human` switches to human-readable tables or cards.
- `inspect --full` and `who --full` expose all arrays or per-fact rows.
- `ls -H` uses a `Task` column and type badges when subagents are present.
- `inspect -H` shows a subagent table for coordinator sessions.
- `transcript <id>` is path-first by default and returns JSON metadata; `--stdout` prints markdown.

## AX Error Handling

Every gaal error is designed to teach calling agents.

1. What went wrong.
2. A working example.
3. A hint for the next step.

Exit codes are meaningful and consistent: 0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error. `-H` routes errors through human formatting.

## Agent Consumption Notes

**Pipe gotcha with `gaal who`:** The `who` verb consumes trailing args greedily. Capture to a variable first, then pipe.

```bash
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.'
```

**jq assertion pattern:**

```bash
gaal ls --limit 1 | jq -e '.sessions | length == 1 and all(.[]; .id and .engine)' > /dev/null
```

**Composable pipeline:**

```bash
gaal ls --since today | jq -r '.sessions[].id' | xargs -I{} gaal inspect {} --files write
```

**Transcript behavior:** `gaal transcript <id>` returns path metadata by default. Use `--stdout` only when you explicitly want markdown content in the calling context.

## Anti-Patterns

| Do NOT | Do instead |
|--------|------------|
| Pipe `gaal who` directly with `|` | Capture to a variable first, then pipe |
| Assume `gaal ls` has `--status active` or a separate `active` command | Use `gaal ls --all` plus `gaal inspect <id>` |
| Read entire session JSONL manually | Use `gaal inspect <id> --trace`, `gaal inspect <id> --source`, or `gaal transcript <id>` |
| Call `gaal inspect` in a loop for multiple IDs | Use `gaal inspect --ids a1b2,c3d4` |
| Assume `gaal recall` works without handoffs | Check `gaal index status` first |
| Run `gaal create-handoff` without agent-mux installed | Verify agent-mux availability first |

## Security / Approval

- Read-only by default. `create-handoff`, `index backfill`, `index reindex`, `index prune`, `index import-eywa`, and `tag` mutate state.
- `create-handoff` dispatches to LLM via agent-mux or openrouter. Costs money. Use `--dry-run` for batch runs.
- `index backfill` is safe. It reads JSONL and writes to `~/.gaal/`.
- `index import-eywa` is safe. It copies legacy data into gaal's own directory.

## Bundled Resources

| Path | What | When to load |
|------|------|-------------|
| [`DOCS.md`](/Users/otonashi/thinking/building/gaal/DOCS.md) | Canonical user doc with current commands, flags, and examples | Need exact flag behavior or output shapes |
| `references/exit-codes.md` | Exit code table with agent response guidance | Handling errors in scripts or pipelines |
| `references/troubleshooting.md` | Known bugs and workarounds | Something unexpected happens |

## Build

```bash
cargo build --release
```

The installed binary is expected to be `target/release/gaal`. Always verify changes against the release build before treating them as shipped.

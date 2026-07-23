# Verb Reference — gaal

Verified flag reference for the current `gaal` binary.

The goal here is accuracy over exhaustiveness. If a flag or output shape is
not listed below, do not assume it exists just because an older doc mentioned
it.

---

## Commands

Current top-level commands:

1. `ls` — fleet view across indexed sessions
2. `inspect` — session details with focused views; **formerly `show`**
3. `transcript` — rendered transcript markdown path or stdout dump
4. `activity` — source-backed historical activity slices
5. `who` — inverted queries
6. `search` — full-text search over indexed facts
7. `recall` — handoff retrieval for continuity
8. `salt` — self-identification token
9. `find-salt` — visible source-artifact discovery by salt
10. `create-handoff` — LLM extraction into handoff markdown
11. `index` — index maintenance
12. `tag` — apply/remove tags
13. `resolve` — resolve a session reference to source and artifact paths

There is **no separate `active` command**. `activity` is historical and source-backed; it is not process monitoring.

---

## Engine and Source Support

`gaal index backfill` discovers supported local agent trace artifacts, parses
them into normalized sessions and facts, stores those rows in SQLite, and
rebuilds Tantivy full-text search from the indexed facts when new sessions are
indexed. It does not index arbitrary files or directories outside supported
source shapes.

Supported source shapes:

| Engine | Source artifact | Boundary |
|--------|-----------------|----------|
| Codex CLI | `~/.codex/sessions/**/rollout-*.jsonl` | JSONL rollout traces. Codex UI/web history is unsupported unless it writes this local trace shape. |
| Claude Code | `~/.claude/projects/<project>/*.jsonl` plus parent-linked `subagents/agent-*.jsonl` | JSONL Claude Code traces. Claude desktop/web history is unsupported unless it writes this local trace shape. |
| Gemini CLI | `~/.gemini/tmp/*/chats/session-*.json` | Single JSON session files, re-parsed as a whole. |
| Antigravity CLI (`agy`) | `~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`, falling back to `transcript.jsonl` | Native but experimental JSONL transcript support; no agy SQLite/blob parsing. |
| Hermes Agent | `HERMES_STATE_DB`, `HERMES_HOME/state.db`, or `~/.hermes/state.db` | Logical sessions parsed from the Hermes SQLite state DB. |
| Grok Build | `${GROK_HOME:-~/.grok}/sessions/<encoded-cwd>/<full-uuid>/` | Multi-file session directory; `updates.jsonl` is primary and `chat_history.jsonl` is fallback. |

Indexed facts include prompts, replies, commands, file reads/writes, errors,
git operations, and task-spawn records where the source exposes them. Subagent
relationships are parsed only where trace schemas expose usable evidence:
Claude parent `toolUseResult`/Agent events and child JSONL files, Codex
`forked_from_id`/subagent source metadata plus parent spawn/wait/close events,
and Hermes parent session IDs as lineage metadata. Gaal does not infer complete
lineage when the source trace does not contain it.

---

## 1. `ls`

Fleet view across sessions.

### Flags

| Flag | Meaning |
|------|---------|
| `--engine <ENGINE>` | Filter by `claude`, `codex`, `gemini`, `agy`, `hermes`, or `grok` |
| `--session-type <TYPE>` | Filter by `coordinator`, `standalone`, or `subagent` |
| `--subagent-type <TYPE>` | Filter by subagent type (e.g. `gsd-heavy`, `gsd-coordinator`, `Explore`) |
| `--since <SINCE>` | Lower bound; supports durations/dates such as `1d` or `2026-03-01` |
| `--before <BEFORE>` | Upper bound date/time |
| `--cwd <CWD>` | Restrict by working directory substring |
| `--tag <TAG>` | Restrict by tag (repeatable; AND logic) |
| `--sort <SORT>` | `started`, `ended`, `tokens`, `cost`, or `duration` |
| `--limit <LIMIT>` | Max number of results |
| `--aggregate` | Return totals instead of individual sessions |
| `--all` | Include noisy sessions (0 tool calls and <30s duration) |
| `--skip-subagents` | Hide subagent sessions, show only standalone/coordinator |
| `-H, --human` | Human-readable output |

### Notes

- Default JSON output is an object with session metadata plus a `sessions` array.
- In normal mode `gaal ls` hides noise; use `--all` to see everything.
- Use `--aggregate` for token totals and grouped counts.

### Examples

```bash
gaal ls --engine claude --since 3d --limit 5 -H
gaal ls --session-type coordinator --since 1d -H
gaal ls --subagent-type gsd-heavy --since 3d
gaal ls --since 2026-03-20 --before 2026-03-21 --all
gaal ls --aggregate --since 7d
```

---

## 2. `inspect`

Session details with optional focused views. This is the command that replaced
older `show` docs.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Full session ID, unique prefix, registered alias, or `latest` |
| `--files [read\|write\|all]` | File-ops view; bare `--files` defaults to `all` |
| `--errors` | Errors and non-zero exits only |
| `--commands` | Commands only |
| `--git` | Git operations only |
| `--tokens` | Token usage breakdown |
| `--trace` | Full event timeline |
| `--source` | Source-artifact diagnostics and path; a Grok source is a session directory |
| `--include-empty` | Include empty/low-signal subagents in coordinator views |
| `--ids <IDS>` | Batch IDs in comma-delimited form |
| `--tag <TAG>` | Batch filter by tag |
| `-F, --full` | Include full arrays and detail fields |
| `-H, --human` | Human-readable output |

### Notes

- `gaal inspect latest` returns a compact operational/session summary in JSON.
- Focus flags such as `--files`, `--errors`, or `--commands` narrow the output.
- `gaal inspect --ids ...` is the batch-friendly replacement for looping over
  old `show` calls.

### Examples

```bash
gaal inspect latest
gaal inspect latest --files write
gaal inspect 249aad1e --trace
gaal inspect --ids a1b2c3d4,e5f6g7h8 --files read
```

---

## 3. `transcript`

Rendered transcript markdown access. This replaced older `inspect --markdown`
style behavior.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Full session ID, unique prefix, registered alias, or `latest` |
| `--force` | Re-render even if cached markdown exists |
| `--stdout` | Dump markdown to stdout instead of returning path metadata |
| `-H, --human` | Human-readable output |

### Notes

- Default behavior is **path-first**: JSON with transcript path, size, and
  estimated token count.
- Use `--stdout` only when you explicitly want the markdown content in the
  current calling context.

### Examples

```bash
gaal transcript latest
gaal transcript 249aad1e
gaal transcript latest --stdout
```

---

## 4. `activity`

Source-backed historical activity slices across a time window.

### Flags

| Flag | Meaning |
|------|---------|
| `--since <SINCE>` | Lower bound; default `1d`; supports durations, dates, and RFC3339 |
| `--before <BEFORE>` | Upper bound; default now; windows are `[since,before)` |
| `--engine <ENGINE>` | Filter by `claude`, `codex`, `gemini`, `agy`, `hermes`, or `grok` |
| `--cwd <CWD>` | Restrict by working directory substring |
| `--session <ID>` | Render one resolved session only |
| `--skip-subagents` | Hide subagent sessions |
| `--limit <LIMIT>` | Max DB candidates to render |
| `--stdout` | Print markdown bundle instead of JSON path metadata |
| `--force` | Accepted for cache parity; activity output is regenerated |
| `-H, --human` | Human-readable metadata |

### Notes

- `activity` parses raw session sources after DB candidate discovery.
- Single-session slices keep transcript-shaped sections and add activity frontmatter.
- This command is not live monitoring and does not inspect running processes.

### Examples

```bash
gaal activity --since 1d -H
gaal activity --since 2026-05-25 --before 2026-05-26 --stdout
gaal activity --session 9ad81c91 --since 2026-05-25 --before 2026-05-26
```

---

## 5. `who`

Inverted query: which session did X to Y?

`who` uses executed facts, not planned tool calls. For agy, planner records are
not attribution evidence until a corresponding executed action record exists.
Command success is tri-state; missing agy `exit_code` or `success` metadata is
unknown, not success.

### Verbs

| Verb | Meaning |
|------|---------|
| `read` | File read operations |
| `wrote` | File writes/edits |
| `ran` | Command executions |
| `touched` | Broadest interaction query |
| `changed` | File modifications |
| `deleted` | File deletions or removal commands |

There is **no `installed` verb** in the current binary.

### Flags

| Flag | Meaning |
|------|---------|
| `[VERB]` | One of the verbs above |
| `[TARGET]` | File path, command pattern, or search term |
| `--since <SINCE>` | Lower bound time window |
| `--before <BEFORE>` | Upper bound date/time |
| `--cwd <CWD>` | Restrict by working directory |
| `--engine <ENGINE>` | Restrict by engine |
| `--tag <TAG>` | Restrict by tag |
| `--failed` | For `ran`, show only failed commands |
| `--limit <LIMIT>` | Max number of results |
| `-F, --full` | Full per-fact output |
| `-H, --human` | Human-readable output |

### Example

```bash
OUTPUT=$(gaal who wrote AGENTS.md --since 7d)
echo "$OUTPUT" | jq '.'
```

---

## 6. `search`

Full-text search over indexed facts.

### Flags

| Flag | Meaning |
|------|---------|
| `[QUERY]` | Search query |
| `--since <SINCE>` | Lower bound time window |
| `--cwd <CWD>` | Restrict by working directory |
| `--engine <ENGINE>` | Restrict by engine |
| `--field <FIELD>` | `prompts`, `replies`, `commands`, `errors`, `files`, or `all` |
| `--context <CONTEXT>` | Context lines around each match |
| `--limit <LIMIT>` | Max number of results |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal search "gaussian moat" --field commands --limit 5 -H
```

---

## 7. `recall`

Ranked handoff retrieval for continuity.

### Flags

| Flag | Meaning |
|------|---------|
| `[QUERY]` | Optional topic query |
| `--days-back <DAYS_BACK>` | Recency window in days |
| `--limit <LIMIT>` | Max number of sessions |
| `--format <FORMAT>` | `summary`, `handoff`, `brief`, or `full` |
| `--id <ID>` | Direct handoff lookup by session reference (bypasses search). Supports full ID, unique prefix, registered alias, or `latest` |
| `--substance <SUBSTANCE>` | Minimum substance score |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal recall "peekaboo" --format brief --limit 5
gaal recall --id latest --format handoff
```

---

## 8. `create-handoff`

LLM-powered handoff generation.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Session reference or `today` |
| `--jsonl <JSONL>` | Explicit source artifact path; the legacy flag name also accepts a Grok session directory |
| `--engine <ENGINE>` | Extraction engine: `claude`, `codex`, `gemini`, `agy`, `hermes`, or `grok` |
| `--model <MODEL>` | Extraction model |
| `--prompt <PROMPT>` | Custom prompt path |
| `--provider <PROVIDER>` | `agent-mux` or `openrouter`; `agent-mux` is the supported real-execution provider unless dry-run reports another provider as supported |
| `--format <FORMAT>` | Output format; default is `markdown` |
| `--batch` | Batch mode |
| `--since <SINCE>` | Lower bound for batch candidates |
| `--parallel <PARALLEL>` | Max concurrent batch workers |
| `--min-turns <MIN_TURNS>` | Minimum turns required for batch candidates |
| `--this` | Compatibility no-op while parent-session preference is disabled |
| `--effort <EFFORT>` | Effort level: `low`, `medium`, `high`, `xhigh`. Overrides config |
| `--dry-run` | Preview candidates without processing |
| `-H, --human` | Human-readable output |

Single-session dry-run JSON reports both `estimated_llm_calls` and
`worst_case_llm_calls`, the per-session call ceiling, and the resolved
provider/grace/wrapper timeouts. Chunked execution limits reducer validation
retries to the calls left under that ceiling.

### Examples

```bash
gaal create-handoff 249aad1e --dry-run
gaal create-handoff 249aad1e  # only after review and a continuity need
gaal create-handoff --batch --since 1d --dry-run
gaal create-handoff --jsonl "$JSONL" --dry-run
```

---

## 9. `index`

Index maintenance commands.

### Subcommands

| Subcommand | Meaning |
|------------|---------|
| `backfill` | Discover and index supported local trace artifacts (JSONL, JSON, Hermes SQLite) |
| `status` | Show index health/status |
| `reindex` | Force re-index of one session |
| `prune` | Remove old facts before a date |
| `recover-orphans` | Scan for orphaned subagent JSONL files and create ghost parent records |

### Notes

- First run performs a full discovery pass across supported engines unless
  `--engine` is supplied. This includes Grok by default; `--engine grok` is a
  scoped filter, not an opt-in requirement. Later runs use per-engine mtime cursors and skip
  unchanged sessions when possible.
- Claude and Codex JSONL can use byte-offset incremental parsing. Gemini JSON,
  agy transcript JSONL, Hermes SQLite, and Grok multi-file session directories
  are re-parsed as complete source artifacts when they need indexing.
- `--with-markdown` additionally writes deterministic session markdown; it is
  not required for SQLite/Tantivy indexing.

### Example

```bash
gaal index status -H
gaal index backfill
gaal index reindex 249aad1e
```

---

## 10. `tag`

Apply or remove tags on a session.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Session reference, or `ls` to list tags |
| `[TAGS]...` | Tags to add/remove |
| `--remove` | Remove tags instead of adding them |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal tag 249aad1e "research"
gaal tag 249aad1e --remove "research"
gaal tag ls
```

---

## 11. `salt`

Generate a random salt token for self-identification.

### Example

```bash
gaal salt
```

---

## 12. `find-salt`

Find the first visible source artifact containing the provided salt token.

Scans Claude Code, Codex, Antigravity brain JSONL logs, and Grok visible session
sources. Agy and Grok match only executed action/tool output records and ignore
user prompt echoes. For Grok, the legacy `jsonl_path` output field contains the
session directory.

### Flags

| Flag | Meaning |
|------|---------|
| `[SALT]` | Salt token to search for |
| `-H, --human` | Human-readable output |
| `--engine <ENGINE>` | Restrict scan to `claude`, `codex`, `agy`, or `grok` |

### Example

```bash
gaal find-salt GAAL_SALT_abc123    # use the literal token from `gaal salt`, never a shell variable
```

---

## 13. `resolve`

Resolve a full session ID, unique prefix, registered alias, or `latest` to
source and artifact paths plus metadata.

### Flags

| Flag | Meaning |
|------|---------|
| `--engine <ENGINE>` | Filter by `claude`, `codex`, `gemini`, `agy`, `hermes`, or `grok` to disambiguate |

### JSON Output

| Field | Meaning |
|-------|---------|
| `session_id` | Full session ID |
| `short_id` | Resolved display/artifact ID; Hermes uses its registered alias, while Grok keeps the canonical full UUID |
| `engine` | Source engine |
| `jsonl_path` | Legacy field containing the source artifact path; a Grok value is a session directory |
| `transcript_path` | Full path to the rendered transcript markdown |
| `transcript_exists` | Whether the transcript file exists |
| `handoff_path` | Full path to the handoff markdown |
| `handoff_exists` | Whether the handoff file exists |
| `session_type` | Session taxonomy value |
| `model` | Recorded model name |

### Examples

```bash
gaal resolve dc5e98dc
gaal resolve dc5e98dc -H
gaal resolve dc5e98dc --engine claude
```

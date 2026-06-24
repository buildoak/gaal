# Formats Reference

This page describes the output shapes, mode switches, and exit codes that agents and humans should expect from `gaal`.

## Output Modes

### JSON (default)

Most commands emit JSON by default. Exceptions:

- `salt` prints a raw token string
- `recall` prints help and exits successfully when no query is given
- `transcript` prints markdown when `--stdout` is used
- `activity` prints markdown when `--stdout` is used

### Human Mode (`-H` / `--human`)

Most commands accept `-H` and switch to table or card output.

## recall --format Comparison

Use `gaal recall ... --format <name>` to control how much handoff material is returned.

| Format | What it returns | When to use | Token cost |
|--------|----------------|-------------|------------|
| `brief` (default) | Compact session summary blocks | Agent retrieval, quick overview | Low (~500 tokens) |
| `summary` | Structured fields only (`headline`, `projects`, `keywords`, `substance`) | Programmatic parsing, comparison | Low |
| `handoff` | Structured summary + raw handoff content | Full context recovery | Medium |
| `full` | Summary + handoff + files + errors | Deep investigation | High |
| `eywa` | Legacy markdown-oriented layout | Backwards compatibility with eywa consumers | Medium |

## JSON Error Format

After CLI parsing succeeds, command errors include these fields:

```json
{"ok": false, "error": "...", "hint": "...", "example": "...", "exit_code": N}
```

Human mode (`-H`) routes through `format_human()` which renders:

- What went wrong: `<specific problem>`
- Example: `<correct invocation>`
- Hint: `<what to try next>`

Argument parsing errors are emitted by the CLI parser before Gaal's JSON error formatter runs, so they may be plain text rather than this JSON shape.

## Command Status Semantics

Command success is tri-state in structured facts: `true`, `false`, or unknown.
For agy, missing `exit_code` and missing `success` mean unknown, not success.
Explicit `success: true`, `success: false`, or `exit_code` fields win over
exit-like text in command output.

Planned tool calls are not attribution facts. `who`, search, activity, and file
or command counts are driven by executed records. For agy, a `PLANNER_RESPONSE`
may describe intended calls, but facts are emitted from executed action records
such as `RUN_COMMAND`, `VIEW_FILE`, `LIST_DIRECTORY`, `GREP_SEARCH`,
`SEARCH_WEB`, and `GENERATE_IMAGE`.

## Exit Code Reference

| Code | Meaning | Agent response |
|------|---------|----------------|
| `0` | Success | Process output normally |
| `1` | No results | Widen search/filter parameters |
| `2` | Ambiguous ID | Provide a longer ID prefix |
| `3` | Not found | Verify the ID exists with `gaal ls` |
| `10` | Missing index | Run `gaal index backfill` |
| `11` | Parse error | Check input format |

## inspect Output Shapes

Default behavior:

- Compact session card with counts, token summary, tags, and `session_type`

Focused flags:

- `--files`, `--commands`, `--errors`, `--git`, `--trace` swap in specific payloads

Batch mode:

- `--ids`, `--tag` return an array

Human mode:

- Card view with subagent table for coordinators

## ls Output Envelope

Default response fields:

- `query_window`
- `filter`
- `shown`
- `total`
- `total_unfiltered`
- `sessions` array

Aggregate mode returns totals and engine buckets instead of session rows.

## activity Output Envelope

Default response fields:

- `path`
- `size_bytes`
- `estimated_tokens`
- `warning`
- `query_window` with `[since,before)` semantics
- `sessions_rendered`
- `skipped`
- `degraded`

`--stdout` prints a markdown activity bundle. Each included session slice is rendered from raw source and keeps transcript-shaped sections with extra activity frontmatter.

## Time Filters

Most query commands accept:

- Relative: `1h`, `7d`, `2w`, `today`
- Absolute dates: `2026-03-29`
- Timestamps: RFC3339 or `YYYY-MM-DDTHH:MM`

## Noise Filtering

By default `ls` hides sessions with zero tool calls and duration under 30s. Use `--all` to include.

## Gemini Session JSON Format

Gemini sessions are stored as a single JSON file (not JSONL). The root object contains:

```json
{
  "sessionId": "...",
  "startTime": "<ISO8601>",
  "summary": "...",
  "messages": [...]
}
```

Key root-level fields:

- `sessionId` — session identifier
- `startTime` — session start timestamp
- `summary` — root-level session summary; gaal extracts this as a `Summary` event so it appears in session headlines
- `messages` — array of turn objects

### Message Types

Each message has a `type` field:

| `type` | Description |
|--------|-------------|
| `user` | User turn; `content` is an array of `{text}` objects |
| `gemini` | Assistant turn; has `content`, `thoughts`, `toolCalls`, `tokens`, `model` |
| `info` | System info; cancellation signals map to `StopSignal`, others surface as system notes |
| `warning` | System warning; surfaces as a system note |
| `error` | Error signal; maps to `StopSignal` |

### Gemini Turn Fields

A `gemini` message object:

```json
{
  "type": "gemini",
  "timestamp": "<ISO8601>",
  "model": "gemini-2.5-pro",
  "content": "assistant text here",
  "thoughts": [
    { "subject": "Reasoning about X", "description": "..." }
  ],
  "toolCalls": [...],
  "tokens": { "input": 1000, "output": 200, "cached": 50, "thoughts": 30 }
}
```

Thought blocks (`thoughts`) are Gemini's reasoning/thinking steps. Each has a `subject` and `description`. gaal prepends them to the assistant content as `[Thought: {subject}] {description}` so they appear in transcripts and search.

Token fields: `input`, `output`, `cached` (cache read tokens), `thoughts` (reasoning tokens).

### Tool Calls

`toolCalls` is an array of:

```json
{
  "id": "tool-call-id",
  "name": "read_file",
  "args": { "path": "src/main.rs" },
  "status": "success",
  "result": [{ "functionResponse": { "response": { "output": "..." } } }]
}
```

Tool names use Gemini's snake_case naming. gaal normalizes them to the canonical names used by Claude/Codex:

| Gemini raw name | Normalized |
|-----------------|------------|
| `read_file`, `read_many_files` | `Read` |
| `write_file` | `Write` |
| `replace`, `edit_file` | `Edit` |
| `run_shell_command` | `Bash` |
| `list_directory`, `glob` | `Glob` |
| `grep_search` | `Grep` |
| `google_web_search` | `WebSearch` |
| `web_fetch` | `WebFetch` |
| `write_todos` | `WriteTodos` |
| `save_memory` | `SaveMemory` |
| `get_internal_docs` | `GetInternalDocs` |
| `update_topic` | `UpdateTopic` |
| `complete_task` | `CompleteTask` |
| `ask_user` | `AskUser` |
| `cli_help` | `CliHelp` |
| `codebase_investigator` | `CodebaseInvestigator` |
| `activate_skill` | `ActivateSkill` |
| `enter_plan_mode` | `EnterPlanMode` |
| `exit_plan_mode` | `ExitPlanMode` |
| Unknown names | passed through unchanged |

`status` is `"success"` or any other string for error. `result` contains `functionResponse.response.output` (success) or `functionResponse.response.error` (failure).

### Incremental Indexing

Gemini stores each session as a single JSON object, so offsets are not meaningful. gaal re-parses the full file on each incremental index run.

## Agy Antigravity CLI JSONL Format

Agy sessions are native but experimental Gaal engine sources, independent of
agent-mux. The supported contract is current Antigravity transcript JSONL plus
fixture-backed copied JSONL. Discovery reads:

`~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`

If `transcript_full.jsonl` is absent, Gaal falls back to `transcript.jsonl` in
the same logs directory.

The indexed agy session ID is the first 8 characters of the Antigravity brain
UUID, matching Gemini-style short IDs. The full UUID remains recoverable from
the source path.

Hermes is different: `sessions.id` stores the full native Hermes session ID,
and `session_aliases` stores the deterministic 8-character alias used for
lookup and registered transcript/handoff filenames. Do not infer a Hermes short
ID from the first 8 characters of the native ID.

Engine detection for JSONL is also content-based: copied agy transcripts outside
the native brain directory still parse as agy when their records match the
Antigravity schema. Native brain UUIDs take precedence for IDs; copied
transcripts can fall back to content ID fields when present.

Agy transcript records are JSONL events with step metadata, source/type/status
fields, optional text content, and optional tool calls. Gaal normalizes file,
shell, web, and image-generation activity into the same session/fact model used
by other engines.

Runtime support for agy is best-effort and advisory. It uses `created_at`
timestamps and executed agy action records; planner-only records do not prove
live activity or attribution.

Current caveats:

- No SQLite blob parsing for agy; the transcript JSONL is the source of truth.
- Token and cost parity for agy is out of scope.
- SQLite/blob sidecars for agy are out of scope.
- Optional agent-mux sidecar metadata can fill model or missing cwd when a
  matching agy sidecar exists; Gaal does not require agent-mux.
- Image generation is indexed and rendered through normalized tool facts and
  transcript evidence; Gaal does not ingest generated image files.

## Search Index Rebuild Triggers

These commands rebuild Tantivy:

- `index backfill` (only when at least one engine indexed new sessions on that run)
- `index reindex`
- `index prune`
- `index import-eywa`

## Codex Subagent JSONL Schema

Codex subagent tracking uses two JSONL surfaces: the child rollout's own
`session_meta` record for identity, and the parent rollout's `response_item`
function-call records for lifecycle metadata.

### Child Side: `session_meta`

Each spawned child rollout should carry a `session_meta` record near the head of
its JSONL:

```json
{"type":"session_meta","payload":{"id":"019d261e-6e93-78d0-8f2c-29279b9e8252","forked_from_id":"019d261d-dffa-7d21-b0df-5893b4ca9aaf","source":{"subagent":{"role":"worker","nickname":"Atlas"}},"model":"gpt-5.4","cwd":"/home/alex/src/gaal"}}
```

Interpretation:

- `payload.id` is the child session ID.
- `payload.forked_from_id` identifies the parent session.
- `payload.source.subagent.role` is the child-side subagent role hint.
- `payload.source.subagent.nickname` is the child-side nickname hint.
- `payload.model` and `payload.cwd` describe the child rollout environment.

### Parent Side: `spawn_agent`

The parent rollout records subagent creation as a `response_item` with
`payload.type = "function_call"`, followed by a matching
`function_call_output` that returns the created agent ID.

Spawn request:

```json
{"type":"response_item","payload":{"type":"function_call","name":"spawn_agent","call_id":"call_spawn","arguments":"{\"agent_type\":\"worker\",\"message\":\"Investigate the failing index path\"}"}}
```

Spawn result:

```json
{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_spawn","output":"{\"agent_id\":\"019d2e57-8e18-7851-bbc1-93c2458fb749\",\"nickname\":\"Atlas\"}"}}
```

Interpretation:

- `payload.call_id` links the output back to the original `spawn_agent` call.
- `arguments.agent_type` is the requested Codex subagent role.
- `arguments.message` is the prompt sent to the child.
- `output.agent_id` is the durable child identifier returned to the parent.

### Parent Side: `close_agent`

The parent records subagent shutdown the same way: a `close_agent` function call
plus a paired output record.

Close request:

```json
{"type":"response_item","payload":{"type":"function_call","name":"close_agent","call_id":"call_close","arguments":"{\"target\":\"019d2e57-8e18-7851-bbc1-93c2458fb749\"}"}}
```

Close result:

```json
{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_close","output":"{\"previous_status\":{\"completed\":\"done\"}}"}}
```

Interpretation:

- `arguments.target` names the child agent being closed.
- `output.previous_status` reports the status observed before shutdown.
- As with `spawn_agent`, `call_id` is the join key between request and output.

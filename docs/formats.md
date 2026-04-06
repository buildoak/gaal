# Formats Reference

This page describes the output shapes, mode switches, and exit codes that agents and humans should expect from `gaal`.

## Output Modes

### JSON (default)

Most commands emit JSON by default. Exceptions:

- `salt` prints a raw token string
- `recall` prints help and exits successfully when no query is given
- `transcript` prints markdown when `--stdout` is used

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

All errors include these fields:

```json
{"ok": false, "error": "...", "hint": "...", "example": "...", "exit_code": N}
```

Human mode (`-H`) routes through `format_human()` which renders:

- What went wrong: `<specific problem>`
- Example: `<correct invocation>`
- Hint: `<what to try next>`

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
| Unknown names | passed through unchanged |

`status` is `"success"` or any other string for error. `result` contains `functionResponse.response.output` (success) or `functionResponse.response.error` (failure).

### Incremental Indexing

Gemini stores each session as a single JSON object, so offsets are not meaningful. gaal re-parses the full file on each incremental index run.

## Search Index Rebuild Triggers

These commands rebuild Tantivy:

- `index backfill`
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
{"type":"session_meta","payload":{"id":"019d261e-6e93-78d0-8f2c-29279b9e8252","forked_from_id":"019d261d-dffa-7d21-b0df-5893b4ca9aaf","source":{"subagent":{"role":"worker","nickname":"Atlas"}},"model":"gpt-5.4","cwd":"/Users/otonashi/thinking/building/gaal"}}
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

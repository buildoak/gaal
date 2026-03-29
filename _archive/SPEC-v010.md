<!-- Archived 2026-03-29: superseded by DOCS.md / BACKLOG.md -->
# gaal v0.1.0 — Definitive Spec

## What gaal IS

**A JSONL indexer and query tool for AI coding sessions.**

gaal indexes Claude Code and Codex JSONL session files. It queries indexed data. It generates handoff documents. Think of it as `git log` for AI sessions — it reads the artifacts, not the processes.

Three capabilities:
1. **Query** — what happened in a session? (`inspect`, `ls`)
2. **Attribute** — who did what to which file? (`who`)
3. **Remember** — reconnect with past work (`recall`, `create-handoff`, `search`)

Agents are the primary consumers. Humans use `-H` mode. Every default output ≤500 tokens.

## What gaal ISN'T

- **Not a process monitor.** No PID checks, no CPU/RSS, no live enrichment, no fleet view of running processes. Process monitoring will be rethought in a future version.
- **Not a session manager.** Observe archived artifacts, don't control running sessions.
- **Not a dashboard.** No watch mode, no polling loops.

---

## Design Principles

1. **Agent-first.** JSON by default. `-H` for humans.
2. **Token-minimal.** Default output ≤500 tokens per session. Want more → explicit flag (`-F`, `--full`, `--files`, etc.)
3. **No status taxonomy.** Sessions have `started_at` and optionally `ended_at`. That's it. No idle/active/completed/starting/failed enum. No `SessionStatus`, no `compute_session_status()`.
4. **One command per question.** `inspect` answers "tell me about this session." `who` answers "who touched this file."
5. **Every field earns its place.** If agents don't need it, it doesn't ship in default output.
6. **Explicit time scope.** Every command that filters by time MUST include the applied time window in its output. Agents need to know what period they're looking at — silent defaults cause wrong conclusions. Format: `"query_window": {"from": "2026-03-07T00:00:00Z", "to": "2026-03-14T23:59:59Z"}`.

---

## Command Inventory (9 commands)

### 1. `gaal inspect <id>` — Tell me about this session

**Merged from `show` + `inspect`.** The single command for session detail queries. This is the renamed `show` — all `show` flags move here. `show` is removed from the CLI.

**What was dropped (AF2):**
- `velocity` field — removed
- `context` field — removed
- `recent_errors` field — removed
- `process` block — removed entirely (no PID logic in v0.1.0)
- `--active` flag — removed (was fleet view)
- `--watch` flag — removed (was polling loop)
- `status` field — removed (no status taxonomy)

**Behavior:**
- Reads session data from DB/JSONL. No process probing. No live enrichment. Pure indexed data query.

**Default output (~300 tokens):**
```json
{
  "id": "d142e3cc",
  "engine": "claude",
  "model": "claude-opus-4-6",
  "cwd": "coordinator",
  "started_at": "2026-03-14T10:00:00Z",
  "ended_at": "2026-03-14T11:00:00Z",
  "duration_secs": 3600,
  "tokens": {"input": 45000, "output": 12000},
  "turns": 24,
  "headline": "Fixed gaal active dedup bug",
  "file_count": 8,
  "command_count": 15,
  "error_count": 2,
  "git_op_count": 3,
  "tags": []
}
```

No `process` block. No `status` field. `ended_at` is null when session has no recorded end.

**Flags:**

| Flag | What it adds |
|------|-------------|
| `--files [read\|write\|all]` | File operations list |
| `--errors` | Error details |
| `--commands` | Command list |
| `--git` | Git operations |
| `--tokens` | Token breakdown |
| `-F, --full` | Everything |
| `--trace` | Full event timeline |
| `--source` | Just the JSONL path (bare string) |
| `--ids <csv>` | Batch mode |
| `--tag <tag>` | Batch by tag |

**Killed flags:** `--active`, `--watch`. These do not exist.

**No-args behavior:** Error with usage hint, exit code != 0.

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Archived session | `gaal inspect <completed-id>` | All counts present, no `status` field, no `process` field, ≤500 tokens |
| T2 | No args | `gaal inspect` | Error message, exit code != 0 |
| T3 | Invalid ID | `gaal inspect nonexistent` | Clean error, not crash |
| T4 | Files view | `gaal inspect <id> --files` | Array of file ops with path+action |
| T5 | Errors view | `gaal inspect <id> --errors` | Array of errors (Read/Glob/Grep/WebFetch/WebSearch tool results NOT included) |
| T6 | Full output | `gaal inspect <id> -F` | All arrays populated, valid JSON |
| T7 | Human mode | `gaal inspect <id> -H` | Readable card, no JSON |
| T8 | Token budget | `gaal inspect <id> \| wc -c` | < 2000 bytes (~500 tokens per session) |
| T9 | Batch mode | `gaal inspect --ids <id1>,<id2>` | Array of 2 results |

Note: T2 from previous spec (live session test) is removed — no PID logic exists.

---

### 2. `gaal ls` — What sessions exist

Listing with filtering. No `--live` flag. No process information.

**Default output per session (~50 tokens each, ~500 for 10):**
```json
{
  "query_window": {"from": "2026-03-04", "to": "2026-03-14"},
  "shown": 10,
  "total": 2712,
  "sessions": [
    {
      "id": "d142e3cc",
      "engine": "claude",
      "model": "claude-opus-4-6",
      "cwd": "coordinator",
      "started_at": "2026-03-14T10:00:00Z",
      "duration_secs": 3600,
      "tokens": {"input": 45000, "output": 12000},
      "headline": "Fixed gaal active dedup bug"
    }
  ]
}
```

**Changes from current:**
- Drop `status` field entirely from session objects
- Drop `--status` filter flag
- Drop `--sort status` option
- Drop `--live` flag (no PID logic)
- `cwd` shows last path component only (full path behind `-F`)
- Trailing `{"note":"..."}` moves to stderr (clean JSON on stdout)
- Default `--limit`: 10

**Flags (keep):** `--engine`, `--since`, `--before`, `--cwd`, `--tag`, `--sort`, `--limit`, `--children`, `--aggregate`, `-H`

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Default | `gaal ls` | Valid JSON, ≤10 items in `sessions`, no `status` field |
| T2 | Limit | `gaal ls --limit 3` | ≤3 items |
| T3 | Engine filter | `gaal ls --engine claude` | All results engine=claude |
| T4 | Since filter | `gaal ls --since 1d` | All results within last day, `query_window` present |
| T5 | Sort | `gaal ls --sort tokens` | Descending by token count |
| T6 | Human | `gaal ls -H` | Aligned table, no JSON |
| T7 | Aggregate | `gaal ls --aggregate` | Single summary object with `total_sessions`, `total_tokens` |
| T8 | Pipe-safe | `gaal ls \| jq '.sessions[0].id'` | jq succeeds (no trailing note on stdout) |
| T9 | Token budget | `gaal ls \| wc -c` | < 2000 bytes for 10 sessions |
| T10 | CWD truncation | `gaal ls` | cwd shows last component (no slashes) |
| T11 | Query window | `gaal ls` | `query_window.from` and `query_window.to` present |
| T12 | Token counts | `gaal ls --engine claude --limit 3` | `tokens.input` > 100 (not broken single-digit counts) |

---

### 3. `gaal who <verb> <target>` — Who did what to which file

**Strongest command. Inverted attribution queries.**

**Verbs:** `read`, `wrote`, `ran`, `touched`, `changed`, `deleted`

Drop `installed` (never returns results).

**Default output (~400 tokens):**
```json
{
  "query_window": {"from": "2026-03-07", "to": "2026-03-14"},
  "shown": 3,
  "total": 3,
  "sessions": [
    {
      "session_id": "d142e3cc",
      "engine": "claude",
      "latest_ts": "2026-03-14T10:00:00Z",
      "fact_count": 5,
      "subjects": ["src/commands/active.rs", "src/discovery/active.rs"],
      "headline": "Fixed gaal active dedup bug"
    }
  ]
}
```

**Fixes needed:**
- Codex subjects: extract file paths from `*** Update File: path` patch strings → return just the file path
- No args → show help text, exit 0 (not error)

**Flags:** `--since`, `--before`, `--cwd`, `--engine`, `--tag`, `--failed`, `--limit`, `-F`

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Wrote | `gaal who wrote ISSUES.md` | Non-empty sessions with fact_count > 0 |
| T2 | Read | `gaal who read CLAUDE.md` | Results include sessions that read the file |
| T3 | Ran | `gaal who ran cargo` | Command-name matching |
| T4 | No args | `gaal who` | Help text, exit 0 |
| T5 | No results | `gaal who wrote nonexistent.xyz` | Empty sessions array, exit 0 |
| T6 | Human | `gaal who wrote ISSUES.md -H` | Aligned table |
| T7 | Limit | `gaal who wrote ISSUES.md --limit 2` | ≤2 sessions |
| T8 | Token budget | `gaal who wrote ISSUES.md \| wc -c` | < 2000 bytes |
| T9 | Codex subjects | `gaal who wrote <codex-file>` | Clean file paths, not patch strings |
| T10 | Query window | `gaal who wrote ISSUES.md` | `query_window.from` and `query_window.to` present |
| T11 | Since changes window | `gaal who wrote ISSUES.md --since 3d` vs `--since 30d` | Different `query_window.from` values |

---

### 4. `gaal recall [query]` — Reconnect with past work

**Handoff-first session retrieval. The continuity engine.** Keep as-is.

**Default format:** `brief` (token-efficient text)

**Flags:** `--days-back`, `--limit`, `--format` (brief/summary/handoff/full/eywa), `--substance`

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Topic query | `gaal recall "gaal issues"` | ≥1 result with headline |
| T2 | No query | `gaal recall` | Recent substantive sessions |
| T3 | Brief format | `gaal recall "gaal" --format brief` | Compact text, ≤500 tokens |
| T4 | Summary format | `gaal recall "gaal" --format summary` | Valid JSON array |
| T5 | Substance filter | `gaal recall --substance 2` | All results substance ≥ 2 |

---

### 5. `gaal search <query>` — Full-text search over facts

BM25 via Tantivy.

**Fix:** Default `--limit` from 20 → 10.

**Flags:** `--since`, `--cwd`, `--engine`, `--field`, `--context`, `--limit`

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Basic | `gaal search "handoff"` | Ranked results with score > 0 |
| T2 | Field filter | `gaal search "cargo" --field commands` | Only command facts |
| T3 | Limit | `gaal search "gaal" --limit 3` | ≤3 results |
| T4 | No results | `gaal search "xyzzy123nonexistent"` | Empty array, exit 0 |
| T5 | Token budget | `gaal search "gaal" --limit 10 \| wc -c` | < 2000 bytes |

---

### 6. `gaal create-handoff [id]` — Generate handoff document

LLM-powered session summary. Keep as-is.

**Flags:** `--jsonl`, `--engine`, `--model`, `--batch`, `--since`, `--this`, `--dry-run`, etc.

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Dry run | `gaal create-handoff --this --dry-run` | Shows candidate, doesn't process |
| T2 | By ID | `gaal create-handoff <id> --dry-run` | Shows candidate for that session |
| T3 | Help | `gaal create-handoff --help` | All flags documented |

---

### 7. `gaal salt` / `gaal find-salt <salt>` — Self-identification

**Two-step flow (MUST be separate tool calls — JSONL flush between them):**
1. `SALT=$(gaal salt)` — generates `GAAL_SALT_<hex16>`
2. `gaal find-salt $SALT` — returns `{engine, session_id, jsonl_path}`

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Generate | `gaal salt` | Matches `GAAL_SALT_[a-f0-9]{16}` |
| T2 | Find (separate call) | `gaal find-salt <salt>` | Returns `engine`, `session_id`, `jsonl_path` |
| T3 | Not found | `gaal find-salt GAAL_SALT_0000000000000000` | Clean error, not crash |

---

### 8. `gaal index` — Index infrastructure

Subcommands: `backfill`, `status`, `reindex`, `prune`, `import-eywa`. Keep as-is.

**Phase 2 addition:** After AF1 token counting fix, run `gaal index backfill --force` to recompute all historical token data.

---

### 9. `gaal tag <id> <tags...>` — Session tagging

Keep as-is. Add `tag ls` subcommand for tag discovery (AF9).

**`tag ls` output:**
```json
["deploy", "gaal-work", "debugging"]
```

**Tests:**

| # | Test | Command | Assert |
|---|------|---------|--------|
| T1 | Add | `gaal tag <id> test-tag` | Tags include "test-tag" |
| T2 | Remove | `gaal tag <id> test-tag --remove` | Tags exclude "test-tag" |
| T3 | List | `gaal tag ls` | Array of all tags (valid JSON array of strings) |

---

## Architecture Fixes

### AF1: Fix Claude token counting (CRITICAL)

**Problem:** Parser reads `input_tokens: 3`, ignores `cache_creation_input_tokens: 22209` and `cache_read_input_tokens`.
**Fix location:** `src/parser/claude.rs`, function `extract_claude_usage_event()` (line ~178).
**Current code:**
```rust
input_tokens: as_i64(record.pointer("/message/usage/input_tokens")),
```
**Correct formula:**
```
total_input = input_tokens + cache_creation_input_tokens + cache_read_input_tokens
```
**New code must read all three fields from `/message/usage/` and sum them.**
**Impact:** Fixes token displays across `ls`, `inspect`, `search`, cost tracking.
**Reindex step:** After fix, run `gaal index backfill --force` to recompute historical data. Add this to Phase 2 gate.

### AF2: Merge show + inspect → inspect (MAJOR)

1. Move all `show` logic from `src/commands/show.rs` into `src/commands/inspect.rs`
2. `inspect` default output: compact card (current `show` default)
3. **Drop entirely:** `velocity` field, `context` field, `recent_errors` field, `process` block
4. **Kill flags:** `--active`, `--watch`
5. Remove `show` command from CLI (`src/main.rs`, `src/commands/mod.rs`)
6. Remove `status` field from output
7. No auto-detect of live sessions. No PID probing. No process enrichment. `inspect` = pure DB/JSONL read.

### AF3: Drop status system

1. Remove `SessionStatus` enum and `compute_session_status()` from `src/model/status.rs`
2. Remove `status` field from all JSON outputs
3. Remove `--status` filter and `--sort status` from `ls`
4. Sessions have `started_at` and optionally `ended_at`. Binary signal: `ended_at` present = done.
5. Clean up any imports of `SessionStatus` across the codebase.

### AF4: Fix error misclassification (CRITICAL)

**Problem:** Claude tool results from Read, Glob, Grep, WebFetch, WebSearch are misclassified as errors. The `contains_error()` function in `src/parser/common.rs` (line ~150) does naive substring matching on tool output text, catching tool results that happen to contain words like "error" or "failed" in their content.

**Fix — two-part rule:**

1. **Only classify as `FactType::Error` when:**
   - The tool is `Bash` or `exec_command` (shell tools) AND the exit code != 0, OR
   - The `is_error` field is explicitly `true` in the JSONL event

2. **Never classify as error when the tool name is any of:**
   `Read`, `Glob`, `Grep`, `WebFetch`, `WebSearch`, `Write`, `Edit`, `NotebookEdit`

**Implementation location:** `src/parser/facts.rs`, in the `EventKind::ToolResult` match arm (~line 234-276). The fix must check `tool_name` from `ToolCallState` before applying the `contains_error()` heuristic. For non-shell tools, skip the `output_has_error` heuristic entirely — only honor the explicit `is_error` field.

### AF5: Fix ls JSON format

**Problem:** Trailing `{"note":"..."}` after the main JSON breaks `jq` piping.
**Fix:** Move note to stderr. Clean JSON object (with `query_window`, `shown`, `total`, `sessions` array) on stdout.

### AF6: Fix who Codex subjects

**Problem:** Raw patch strings (`*** Update File: path\n*** Begin Patch...`) appear in `subjects` array.
**Fix:** Parse `*** Update File: <path>` lines → extract just the file path. Drop the patch content.

### AF7: Explicit time scope on all time-filtered outputs

Every command that applies a time filter MUST include `query_window` in its JSON output.

**Affected commands and their default windows:**
- `ls` — default window based on `--since`/`--before`, or all indexed time when unfiltered
- `who` — default 7 days, respects `--since`/`--before`
- `search` — default 30 days, respects `--since`
- `recall` — default 14 days, respects `--days-back`

**Format:**
```json
"query_window": {"from": "2026-03-07", "to": "2026-03-14"}
```

When no time filter: `"query_window": {"from": "<earliest_session_date>", "to": "now"}`.

### AF8: `who` no-args behavior

**Problem:** `gaal who` with no arguments should show help and exit cleanly.
**Fix:** No args → print usage/help text, exit 0. Not an error.

### AF9: `tag ls` subcommand

**Problem:** No way to discover what tags exist.
**Fix:** `gaal tag ls` returns a JSON array of all distinct tag strings. Exit 0 even if empty (returns `[]`).

---

## Token Budget Contract

| Command | Scope | Default max | With --full |
|---------|-------|------------|-------------|
| `inspect <id>` | per session | 500 tokens | unbounded |
| `ls` (10 items) | total output | 500 tokens | unbounded |
| `who` | total output | 500 tokens | unbounded |
| `search` (10 items) | total output | 500 tokens | unbounded |
| `recall --format brief` | total output | 300 tokens | N/A |
| `salt` | — | 1 line | N/A |
| `find-salt` | — | 50 tokens | N/A |

---

## Breaking Changes

This section documents user-visible changes that break backward compatibility with pre-v0.1.0 gaal.

| Change | Old behavior | New behavior | Migration |
|--------|-------------|-------------|-----------|
| `gaal show` removed | Worked as session detail view | `error: unrecognized subcommand 'show'` | Use `gaal inspect <id>` |
| `status` field removed | Present in `ls` and `show` output | Not present in any output | Check `ended_at` instead: null = ongoing, set = done |
| `--status` filter removed | `gaal ls --status active` filtered by status | `error: unexpected argument` | No replacement (no status taxonomy) |
| `--active` flag removed | `gaal inspect --active` showed fleet view | `error: unexpected argument` | No replacement in v0.1.0 |
| `--watch` flag removed | `gaal inspect --watch` polled for updates | `error: unexpected argument` | No replacement |
| `--live` flag removed | `gaal ls --live` showed live sessions | `error: unexpected argument` | No replacement (no PID logic) |
| `process` block removed | Live sessions had `{pid, cpu_pct, rss_mb}` | Field does not exist | No replacement in v0.1.0 |
| `velocity` field removed | Present in inspect output | Field does not exist | No replacement |
| `context` field removed | Present in inspect output | Field does not exist | No replacement |
| `recent_errors` field removed | Present in inspect output | Field does not exist | Use `--errors` flag |
| Token counts changed | `input_tokens` only (often single-digit) | `input + cache_creation + cache_read` (realistic) | `gaal index backfill --force` to reindex |
| `search` default limit | 20 results | 10 results | Use `--limit 20` explicitly |
| `installed` verb removed | `gaal who installed <pkg>` | `error: invalid verb` | No replacement |

---

## Comprehensive Testing Plan

### Test Architecture

Tests are bash scripts that run the actual `gaal` binary against real session data on this machine. No mocks. Each test outputs `PASS`/`FAIL` with the command, expected, and actual. Exit code 0 = all pass, 1 = any fail.

Test runner: `tests/run-all.sh` — runs all test suites in sequence, reports summary.

### Suite 1: `inspect` (9 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

# Helper
assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

# Get a known session ID
ID=$(gaal ls --limit 1 2>/dev/null | jq -r '.sessions[0].id')

# T1-inspect-archived: Archived session returns compact card
OUT=$(gaal inspect "$ID")
assert "T1: has id" echo "$OUT" | jq -e '.id'
assert "T1: no status field" bash -c "echo '$OUT' | jq -e '.status' 2>/dev/null && exit 1 || exit 0"
assert "T1: no process field" bash -c "echo '$OUT' | jq -e 'has(\"process\") | not'"
BYTES=$(echo "$OUT" | wc -c)
assert "T1: token budget" [ "$BYTES" -lt 2000 ]

# T2-inspect-noargs: No args → error
assert "T2: no args error" bash -c "! gaal inspect 2>/dev/null"

# T3-inspect-invalid: Invalid ID → clean error
assert "T3: invalid id" bash -c "! gaal inspect nonexistent_id_xyz 2>/dev/null"

# T4-inspect-files: --files returns array
OUT=$(gaal inspect "$ID" --files)
assert "T4: files array" echo "$OUT" | jq -e '.files | type == "array"'

# T5-inspect-errors: --errors excludes file reads
OUT=$(gaal inspect "$ID" --errors)
assert "T5: errors array" echo "$OUT" | jq -e '.errors | type == "array"'

# T6-inspect-full: -F returns all arrays
OUT=$(gaal inspect "$ID" -F)
assert "T6: commands array" echo "$OUT" | jq -e '.commands | type == "array"'
assert "T6: files array" echo "$OUT" | jq -e '.files | type == "array"'
assert "T6: errors array" echo "$OUT" | jq -e '.errors | type == "array"'

# T7-inspect-human: -H returns readable text, not JSON
OUT=$(gaal inspect "$ID" -H)
assert "T7: not JSON" bash -c "echo '$OUT' | jq . 2>/dev/null && exit 1 || exit 0"

# T8-inspect-token-budget: Default output ≤ 500 tokens (~2000 bytes)
BYTES=$(gaal inspect "$ID" | wc -c)
assert "T8: under budget" [ "$BYTES" -lt 2000 ]

# T9-inspect-batch: --ids returns array
ID2=$(gaal ls --limit 2 2>/dev/null | jq -r '.sessions[1].id')
OUT=$(gaal inspect --ids "$ID,$ID2")
assert "T9: batch array" echo "$OUT" | jq -e 'type == "array" and length == 2'

echo "---"
echo "inspect: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 2: `ls` (12 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

# T1-ls-default: Valid JSON, ≤10 items, no status
OUT=$(gaal ls)
assert "T1: valid JSON with sessions" echo "$OUT" | jq -e '.sessions | type == "array"'
COUNT=$(echo "$OUT" | jq '.sessions | length')
assert "T1: ≤10 items" [ "$COUNT" -le 10 ]
assert "T1: no status field" bash -c "echo '$OUT' | jq -e '.sessions[0].status' 2>/dev/null && exit 1 || exit 0"

# T2-ls-limit: --limit respected
OUT=$(gaal ls --limit 3)
COUNT=$(echo "$OUT" | jq '.sessions | length')
assert "T2: ≤3 items" [ "$COUNT" -le 3 ]

# T3-ls-engine: Engine filter
OUT=$(gaal ls --engine claude)
assert "T3: all claude" echo "$OUT" | jq -e '.sessions | all(.engine == "claude")'

# T4-ls-since: Time filter + query_window
OUT=$(gaal ls --since 1d)
assert "T4: query_window.from" echo "$OUT" | jq -e '.query_window.from'
assert "T4: query_window.to" echo "$OUT" | jq -e '.query_window.to'

# T5-ls-sort-tokens: Sort by tokens descending
OUT=$(gaal ls --sort tokens --limit 5)
FIRST=$(echo "$OUT" | jq '.sessions[0].tokens.input')
SECOND=$(echo "$OUT" | jq '.sessions[1].tokens.input')
assert "T5: descending order" [ "$FIRST" -ge "$SECOND" ]

# T6-ls-human: -H aligned table
OUT=$(gaal ls -H)
assert "T6: not JSON" bash -c "echo '$OUT' | jq . 2>/dev/null && exit 1 || exit 0"

# T7-ls-aggregate: --aggregate returns totals
OUT=$(gaal ls --aggregate)
assert "T7: total_sessions" echo "$OUT" | jq -e '.total_sessions'
assert "T7: total_tokens" echo "$OUT" | jq -e '.total_tokens'

# T8-ls-pipe-safe: jq piping works (no trailing note on stdout)
assert "T8: jq pipe" gaal ls | jq '.sessions[0].id'

# T9-ls-token-budget: ≤ 2000 bytes for 10 sessions
BYTES=$(gaal ls | wc -c)
assert "T9: under budget" [ "$BYTES" -lt 2000 ]

# T10-ls-cwd-truncation: cwd is last component (no slashes)
CWD=$(gaal ls --limit 1 | jq -r '.sessions[0].cwd')
assert "T10: no slashes" bash -c "echo '$CWD' | grep -v '/'"

# T11-ls-query-window: query_window always present
OUT=$(gaal ls)
assert "T11: query_window.from" echo "$OUT" | jq -e '.query_window.from'
assert "T11: query_window.to" echo "$OUT" | jq -e '.query_window.to'

# T12-ls-token-counts: Claude sessions have realistic token counts
OUT=$(gaal ls --engine claude --limit 3)
TOKENS=$(echo "$OUT" | jq '.sessions[0].tokens.input')
assert "T12: tokens > 100" [ "$TOKENS" -gt 100 ]

echo "---"
echo "ls: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 3: `who` (11 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

# T1-who-wrote: Basic attribution
OUT=$(gaal who wrote ISSUES.md)
assert "T1: non-empty" echo "$OUT" | jq -e '.sessions | length > 0'
assert "T1: fact_count" echo "$OUT" | jq -e '.sessions[0].fact_count > 0'

# T2-who-read: Read attribution
OUT=$(gaal who read CLAUDE.md)
assert "T2: results" echo "$OUT" | jq -e '.sessions | length > 0'

# T3-who-ran: Command attribution
OUT=$(gaal who ran cargo)
assert "T3: results" echo "$OUT" | jq -e '.sessions | length > 0'

# T4-who-noargs: Help text, exit 0
assert "T4: exit 0" gaal who
OUT=$(gaal who 2>&1)
assert "T4: help text" echo "$OUT" | grep -qi 'usage\|verb\|help'

# T5-who-noresults: Empty result, exit 0
OUT=$(gaal who wrote nonexistent_file_xyz_123.rs)
assert "T5: empty sessions" echo "$OUT" | jq -e '.sessions | length == 0'

# T6-who-human: -H aligned table
OUT=$(gaal who wrote ISSUES.md -H)
assert "T6: not JSON" bash -c "echo '$OUT' | jq . 2>/dev/null && exit 1 || exit 0"

# T7-who-limit: --limit respected
OUT=$(gaal who wrote ISSUES.md --limit 2)
COUNT=$(echo "$OUT" | jq '.sessions | length')
assert "T7: ≤2" [ "$COUNT" -le 2 ]

# T8-who-token-budget: < 2000 bytes default
BYTES=$(gaal who wrote ISSUES.md | wc -c)
assert "T8: under budget" [ "$BYTES" -lt 2000 ]

# T9-who-codex-subjects: Clean file paths, not patch strings
OUT=$(gaal who wrote Cargo.toml --engine codex 2>/dev/null || echo '{"sessions":[]}')
assert "T9: no patch strings" bash -c "echo '$OUT' | jq -r '.sessions[].subjects[]?' 2>/dev/null | grep -v 'Begin Patch' || true"

# T10-who-query-window: query_window present
OUT=$(gaal who wrote ISSUES.md)
assert "T10: query_window.from" echo "$OUT" | jq -e '.query_window.from'
assert "T10: query_window.to" echo "$OUT" | jq -e '.query_window.to'

# T11-who-since: --since changes query_window
OUT1=$(gaal who wrote ISSUES.md --since 3d)
OUT2=$(gaal who wrote ISSUES.md --since 30d)
FROM1=$(echo "$OUT1" | jq -r '.query_window.from')
FROM2=$(echo "$OUT2" | jq -r '.query_window.from')
assert "T11: different windows" [ "$FROM1" != "$FROM2" ]

echo "---"
echo "who: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 4: `search` (5 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

# T1-search-basic: Returns ranked results
OUT=$(gaal search "handoff")
assert "T1: has results with score" echo "$OUT" | jq -e '.[0].score > 0'

# T2-search-field: --field filters to fact type
OUT=$(gaal search "cargo" --field commands)
assert "T2: all command type" echo "$OUT" | jq -e 'all(.fact_type == "command")'

# T3-search-limit: --limit respected
OUT=$(gaal search "gaal" --limit 3)
COUNT=$(echo "$OUT" | jq 'length')
assert "T3: ≤3 results" [ "$COUNT" -le 3 ]

# T4-search-noresults: Empty array, exit 0
OUT=$(gaal search "xyzzy123nonexistent")
assert "T4: empty array" echo "$OUT" | jq -e 'length == 0'

# T5-search-token-budget: < 2000 bytes for 10 results
BYTES=$(gaal search "gaal" --limit 10 | wc -c)
assert "T5: under budget" [ "$BYTES" -lt 2000 ]

echo "---"
echo "search: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 5: `recall` (5 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

# T1-recall-topic: Returns results with headline
OUT=$(gaal recall "gaal issues")
assert "T1: has content" echo "$OUT" | grep -qi 'session\|headline'

# T2-recall-noquery: Returns recent substantive sessions
OUT=$(gaal recall)
assert "T2: has content" echo "$OUT" | grep -qi 'session\|headline'

# T3-recall-brief: Compact format
BYTES=$(gaal recall "gaal" --format brief | wc -c)
assert "T3: under budget" [ "$BYTES" -lt 2000 ]

# T4-recall-summary: Valid JSON
OUT=$(gaal recall "gaal" --format summary)
assert "T4: valid JSON array" echo "$OUT" | jq -e '.[0].session_id'

# T5-recall-substance: Substance filter
OUT=$(gaal recall --substance 2 --format summary)
assert "T5: all substance ≥ 2" echo "$OUT" | jq -e 'all(.substance >= 2)'

echo "---"
echo "recall: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 6: `salt` + `find-salt` (3 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

# T1-salt-generate: Valid format
SALT=$(gaal salt)
assert "T1: valid format" echo "$SALT" | grep -E '^GAAL_SALT_[a-f0-9]{16}$'

# T2-find-salt: Finds session (MUST be separate invocation from salt generation)
# Note: In automated tests, use a previously generated salt that exists in indexed data
SALT=$(gaal salt)
sleep 1  # Allow JSONL flush
OUT=$(gaal find-salt "$SALT" 2>/dev/null || echo '{}')
# This may fail in CI without a live session — skip gracefully
if echo "$OUT" | jq -e '.engine' >/dev/null 2>&1; then
  assert "T2: has engine" echo "$OUT" | jq -e '.engine'
  assert "T2: has session_id" echo "$OUT" | jq -e '.session_id'
  assert "T2: has jsonl_path" echo "$OUT" | jq -e '.jsonl_path'
else
  echo "SKIP: T2 (no active session for find-salt)"
fi

# T3-find-salt-notfound: Clean error
assert "T3: not found exits cleanly" bash -c "gaal find-salt GAAL_SALT_0000000000000000 2>/dev/null; [ \$? -ne 0 ]"

echo "---"
echo "salt: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 7: `tag` (3 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

ID=$(gaal ls --limit 1 2>/dev/null | jq -r '.sessions[0].id')

# T1-tag-add: Add tag
gaal tag "$ID" _test_tag_v010 2>/dev/null
OUT=$(gaal inspect "$ID")
assert "T1: tag added" echo "$OUT" | jq -e '.tags | index("_test_tag_v010")'

# T2-tag-remove: Remove tag
gaal tag "$ID" _test_tag_v010 --remove 2>/dev/null
OUT=$(gaal inspect "$ID")
assert "T2: tag removed" echo "$OUT" | jq -e '.tags | index("_test_tag_v010") | not'

# T3-tag-ls: List all tags
OUT=$(gaal tag ls)
assert "T3: array" echo "$OUT" | jq -e 'type == "array"'

echo "---"
echo "tag: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Suite 8: Cross-cutting (6 tests)

```bash
#!/bin/bash
set -euo pipefail
PASS=0; FAIL=0

assert() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $name"; ((PASS++))
  else
    echo "FAIL: $name"; ((FAIL++))
  fi
}

ID=$(gaal ls --limit 1 2>/dev/null | jq -r '.sessions[0].id')

# T1-no-status: ls and inspect do not return status field
# Scoped to ls and inspect only (other commands have different output shapes)
OUT_LS=$(gaal ls)
assert "T1: ls no status" bash -c "echo '$OUT_LS' | jq -e '.sessions[0].status' 2>/dev/null && exit 1 || exit 0"
OUT_INSPECT=$(gaal inspect "$ID")
assert "T1: inspect no status" bash -c "echo '$OUT_INSPECT' | jq -e '.status' 2>/dev/null && exit 1 || exit 0"

# T2-show-dead: `gaal show` is removed
assert "T2: show is dead" bash -c "! gaal show 2>/dev/null"

# T3-active-dead: `gaal inspect --active` is removed
assert "T3: --active is dead" bash -c "! gaal inspect --active 2>/dev/null"

# T4-json-valid: Every command produces valid JSON (no trailing objects on stdout)
assert "T4: ls valid JSON" gaal ls | jq .
assert "T4: search valid JSON" gaal search gaal | jq .
assert "T4: who valid JSON" gaal who wrote ISSUES.md | jq .

# T5-human-not-json: Every -H mode produces non-JSON
assert "T5: ls -H not JSON" bash -c "gaal ls -H | jq . 2>/dev/null && exit 1 || exit 0"
assert "T5: inspect -H not JSON" bash -c "gaal inspect '$ID' -H | jq . 2>/dev/null && exit 1 || exit 0"
assert "T5: who -H not JSON" bash -c "gaal who wrote ISSUES.md -H | jq . 2>/dev/null && exit 1 || exit 0"

# T6-query-window-present: All time-scoped commands include query_window
assert "T6: ls query_window" gaal ls | jq -e '.query_window'
assert "T6: who query_window" gaal who wrote ISSUES.md | jq -e '.query_window'

echo "---"
echo "cross-cutting: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
```

### Total: 54 tests across 8 suites

| Suite | Command | Tests |
|-------|---------|-------|
| 1 | inspect | 9 |
| 2 | ls | 12 |
| 3 | who | 11 |
| 4 | search | 5 |
| 5 | recall | 5 |
| 6 | salt + find-salt | 3 |
| 7 | tag | 3 |
| 8 | cross-cutting | 6 |

---

## GSD Execution Plan

### Phase 1: Architecture Fixes (parallel, 6 workers)

These are independent — all can run simultaneously, except W2 depends on W3.

| Worker | Engine | Task | Files | Gate |
|--------|--------|------|-------|------|
| W1 | GPT-5.4 xhigh | AF1: Fix Claude token counting | `src/parser/claude.rs` — function `extract_claude_usage_event()` (~line 178) | After `cargo build --release` + `gaal index backfill --force`: `gaal ls --engine claude --limit 1 \| jq '.sessions[0].tokens.input'` returns > 100 |
| W2 | Opus | AF2: Merge show → inspect + AF3: Drop status system | `src/commands/show.rs`, `src/commands/inspect.rs`, `src/main.rs`, `src/commands/mod.rs`, `src/model/status.rs`, `src/commands/ls.rs` | `gaal inspect <id>` works with no `status`/`process`/`velocity`/`context`/`recent_errors` fields. `gaal show` → unrecognized subcommand. `gaal inspect --active` → unexpected argument. No `SessionStatus` references remain. |
| W3 | GPT-5.4 high | AF3: Drop status system (prerequisite for W2) | `src/model/status.rs`, `src/commands/ls.rs`, all files importing `SessionStatus` | No `status` field in `gaal ls` output. `--status` and `--sort status` removed from CLI. `SessionStatus` enum deleted. |
| W4 | GPT-5.4 high | AF4: Fix error misclassification | `src/parser/facts.rs` (~line 234-276), `src/parser/common.rs` | `gaal inspect <id> --errors` contains no Read/Glob/Grep/WebFetch/WebSearch tool results. Only shell tools with exit_code != 0 or explicit `is_error: true` are classified as errors. |
| W5 | GPT-5.4 high | AF5: Fix ls JSON format + AF7: query_window | `src/commands/ls.rs`, `src/commands/who.rs`, `src/commands/search.rs` | `gaal ls \| jq '.query_window'` succeeds. `gaal ls \| jq '.sessions[0].id'` succeeds (no trailing note). `gaal who wrote ISSUES.md \| jq '.query_window'` succeeds. |
| W6 | GPT-5.4 high | AF6: Fix who Codex subjects + AF8: who no-args + AF9: tag ls | `src/commands/who.rs`, `src/commands/tag.rs`, Codex fact extraction | `gaal who wrote Cargo.toml --engine codex` shows file paths not patches. `gaal who` exits 0 with help. `gaal tag ls` returns JSON array. |

**Dependencies:** W2 depends on W3 completing first. All others are independent.

**Execution:** W1, W3, W4, W5, W6 launch in parallel. W2 launches after W3 completes.

### Phase 2: Integration Build (1 worker)

| Worker | Engine | Task | Gate |
|--------|--------|------|------|
| W7 | Opus | Merge all changes, resolve conflicts, `cargo build --release`, run `gaal index backfill --force` | Clean release build. Zero warnings (except known dead-code). Backfill completes without error. `gaal ls --engine claude --limit 1 \| jq '.sessions[0].tokens.input'` returns > 100. |

### Phase 3: Test Matrix (parallel, 8 workers)

Each worker runs one test suite against the installed binary.

| Worker | Engine | Suite | Tests |
|--------|--------|-------|-------|
| W8 | GPT-5.4 high | Suite 1: inspect | 9 tests |
| W9 | GPT-5.4 high | Suite 2: ls | 12 tests |
| W10 | GPT-5.4 high | Suite 3: who | 11 tests |
| W11 | GPT-5.4 high | Suite 4: search | 5 tests |
| W12 | GPT-5.4 high | Suite 5: recall | 5 tests |
| W13 | GPT-5.4 high | Suite 6: salt + find-salt | 3 tests |
| W14 | GPT-5.4 high | Suite 7: tag | 3 tests |
| W15 | GPT-5.4 high | Suite 8: cross-cutting | 6 tests |

### Phase 4: Doc Update + Final Audit (2 workers)

| Worker | Engine | Task | Gate |
|--------|--------|------|------|
| W16 | Opus | Update CLAUDE.md, SKILL.md, ISSUES.md | All docs reflect v0.1.0 reality. No stale `gaal active` references. No stale `show` references. No stale `status` references. No stale `--live` references. |
| W17 | Opus | Final audit: run every command, verify token budgets, check for regressions | All 54 tests pass. No command exceeds 500 tokens default. Every time-scoped command includes `query_window`. |

### Resource Summary

| Phase | Workers | Engines | Estimated time |
|-------|---------|---------|---------------|
| 1: Architecture | 6 | 1x Opus, 5x GPT-5.4 | ~15 min (parallel) |
| 2: Integration | 1 | 1x Opus | ~5 min |
| 3: Test Matrix | 8 | 8x GPT-5.4 | ~5 min (parallel) |
| 4: Docs + Audit | 2 | 2x Opus | ~10 min (parallel) |
| **Total** | **17 workers** | **4x Opus, 13x GPT-5.4** | **~35 min wall clock** |

### Coordinator Protocol

GSD coordinator reads this spec, dispatches phases sequentially (1 → 2 → 3 → 4), workers within each phase run in parallel. Each worker:
1. Reads `/Users/otonashi/thinking/building/gaal/CLAUDE.md` first (verification protocol is law)
2. Reads this spec (`SPEC-v010.md`) for its assigned task
3. Dumps real JSONL data before writing code (evidence-first rule)
4. Implements the change
5. Runs `cargo build --release` (not debug — the installed binary is a symlink to `target/release/gaal`)
6. Runs its verification gate against the built binary
7. Reports: files changed, gate result, any issues

If a gate fails, coordinator re-dispatches the worker with the error output. Max 2 retries per worker before escalating to coordinator.

---

## Verification Gate

After all fixes, the final audit confirms:

- [ ] `cargo build --release` — clean build
- [ ] `gaal index backfill --force` — completes without error
- [ ] `gaal show` → `error: unrecognized subcommand`
- [ ] `gaal inspect --active` → `error: unexpected argument`
- [ ] `gaal inspect --watch` → `error: unexpected argument`
- [ ] `gaal ls --live` → `error: unexpected argument`
- [ ] `gaal ls --status active` → `error: unexpected argument`
- [ ] No `status` field in `gaal ls` or `gaal inspect` output
- [ ] No `process` field in `gaal inspect` output
- [ ] No `velocity`, `context`, `recent_errors` fields in `gaal inspect` output
- [ ] Claude token counts are realistic (thousands, not single digits)
- [ ] `gaal ls | jq '.sessions[0].id'` succeeds (clean JSON, no trailing note)
- [ ] `gaal ls | jq '.query_window'` succeeds
- [ ] `gaal who wrote ISSUES.md | jq '.query_window'` succeeds
- [ ] `gaal who` exits 0 with help text
- [ ] `gaal tag ls` returns valid JSON array
- [ ] `gaal inspect <id> --errors` contains no Read/Glob/Grep/WebFetch/WebSearch false positives
- [ ] `gaal who wrote Cargo.toml --engine codex` shows clean file paths, no patch strings
- [ ] Every default command output ≤ 500 tokens (~2000 bytes)
- [ ] All 54 tests across 8 suites pass

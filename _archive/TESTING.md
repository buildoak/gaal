<!-- Archived 2026-03-29: superseded by DOCS.md / BACKLOG.md -->
# TESTING.md — Gaal Agentic Test Plan

20 test cases against real indexed data. No mocks. Every command is copy-paste runnable.

## Prerequisites

- **Binary**: `/opt/homebrew/bin/gaal` installed and on PATH
- **Index**: `~/.gaal/index.db` exists with data (`gaal index status` reports sessions_total > 0)
- **Tantivy**: `~/.gaal/tantivy/meta.json` exists (built by `gaal index backfill`)
- **jq**: Available for JSON assertions (`brew install jq` if missing)
- **Active sessions**: Some tests (T-13, T-14, T-15) require at least one Claude or Codex process running

## How to Run

Each test is independent. Run the command, check exit code and output against Expected.

Quick smoke run (T-01 through T-04):
```bash
for t in \
  "gaal --version" \
  "gaal --help" \
  "gaal index status" \
  "gaal ls --limit 1"; do
  echo "--- $t ---"
  eval "$t" >/dev/null 2>&1 && echo "PASS (exit 0)" || echo "FAIL (exit $?)"
done
```

## Interpreting Results

- **Exit 0**: Success, output produced
- **Exit 1**: No results found (valid for empty queries)
- **Exit 2**: Ambiguous ID prefix
- **Exit 3**: Session not found
- **Exit 10**: No index exists
- **Exit 11**: Parse error / invalid input

When a test says "exit code 0" it means the operation succeeded and produced valid output. JSON shape assertions use `jq` — if `jq` exits non-zero, the shape is wrong.

---

## T-01: Binary exists and reports version
**Level:** Basic
**Verb:** (none — top-level)
**What it tests:** Binary is installed, compiled, and responds to --version
**Command:**
```bash
gaal --version
```
**Expected:** Output matches pattern `gaal 0.1.0` (or higher semver). Exit code 0.
```bash
gaal --version | grep -qE '^gaal [0-9]+\.[0-9]+\.[0-9]+$' && echo PASS || echo FAIL
echo "exit: $?"
```
**Failure signal:** Command not found, or prints nothing, or exit code non-zero.
**Notes:** Catches broken install, missing binary, or dylib issues on macOS after Rust rebuild.

---

## T-02: Help text enumerates all 10 subcommands
**Level:** Basic
**Verb:** (none — top-level)
**What it tests:** Clap CLI wiring — all subcommands registered and reachable
**Command:**
```bash
gaal --help 2>&1
```
**Expected:** Exit code 0. Output contains all 10 subcommands: `ls`, `show`, `inspect`, `who`, `search`, `recall`, `handoff`, `index`, `active`, `tag`. Verify:
```bash
HELP=$(gaal --help 2>&1)
for cmd in ls show inspect who search recall handoff index active tag; do
  echo "$HELP" | grep -qw "$cmd" || echo "MISSING: $cmd"
done
echo "All subcommands present"
```
**Failure signal:** Any subcommand missing from help text means clap wiring is broken for that verb.
**Notes:** Catches accidental removal of a subcommand during refactoring. The enum `Commands` in main.rs must have all 10 variants.

---

## T-03: Index status returns valid JSON with correct schema
**Level:** Basic
**Verb:** `index status`
**What it tests:** SQLite connection opens, schema exists, aggregation queries run
**Command:**
```bash
gaal index status
```
**Expected:** Exit code 0. JSON object with these required keys and types:
```bash
gaal index status | jq -e '
  .db_path and
  (.db_size_bytes | type) == "number" and
  (.sessions_total | type) == "number" and
  (.sessions_by_engine | type) == "object" and
  (.facts_total | type) == "number" and
  (.handoffs_total | type) == "number" and
  has("last_indexed_at") and
  has("oldest_session") and
  has("newest_session")
' && echo PASS || echo FAIL
```
With real data: `sessions_total >= 1000`, `facts_total >= 40000`.
**Failure signal:** Exit 10 (no index), or JSON missing keys, or sessions_total is 0 when data exists.
**Notes:** This is the health check endpoint. If this fails, every other command will also fail.

---

## T-04: ls returns JSON array with correct per-session schema
**Level:** Basic
**Verb:** `ls`
**What it tests:** Default ls query, session row serialization, JSON output format
**Command:**
```bash
gaal ls --limit 3
```
**Expected:** Exit code 0. JSON array of length 3. Each element has these fields:
```bash
gaal ls --limit 3 | jq -e '
  length == 3 and
  all(
    .id and .engine and .model and .status and .cwd and
    .started_at and .duration_secs >= 0 and
    .tokens.input >= 0 and .tokens.output >= 0 and
    .tools_used >= 0 and .child_count >= 0
  )
' && echo PASS || echo FAIL
```
**Failure signal:** Empty array, missing fields, or non-array output.
**Notes:** The `headline` field may be null (no handoffs generated yet). `parent_id` may be null. Both are valid.

---

## T-05: ls with engine filter returns only matching engine
**Level:** Intermediate
**Verb:** `ls`
**What it tests:** Engine filter applied correctly at SQL level
**Command:**
```bash
gaal ls --engine claude --limit 10
```
**Expected:** Exit code 0. Every session has `"engine": "claude"`. Zero codex sessions in output.
```bash
gaal ls --engine claude --limit 10 | jq -e '
  length > 0 and all(.engine == "claude")
' && echo PASS || echo FAIL
```
**Failure signal:** Any session with `engine: "codex"` in the results, or empty result when Claude sessions exist.
**Notes:** Tests the `WHERE s.engine = :engine` clause in `list_sessions`. Also validates the enum conversion from CLI `Engine::Claude` to string `"claude"`.

---

## T-06: show with session ID prefix resolves correctly
**Level:** Intermediate
**Verb:** `show`
**What it tests:** Prefix-based ID resolution via `LIKE` query, single-session output schema
**Command:**
```bash
# Get a known session ID prefix (first 8 chars)
SESSION_ID=$(gaal ls --limit 1 | jq -r '.[0].id')
PREFIX=$(echo "$SESSION_ID" | cut -c1-8)
gaal show "$PREFIX"
```
**Expected:** Exit code 0. JSON object (not array) with full session record. Must contain: `id`, `engine`, `model`, `status`, `cwd`, `started_at`, `duration_secs`, `tokens`, `turns`, `tools_used`, `files`, `commands`, `errors`, `git_ops`, `tags`.
```bash
SESSION_ID=$(gaal ls --limit 1 | jq -r '.[0].id')
PREFIX=$(echo "$SESSION_ID" | cut -c1-8)
gaal show "$PREFIX" | jq -e '
  .id and .engine and .status and
  .files and .commands and .errors and
  .git_ops and (.tags | type) == "array" and
  .turns >= 0
' && echo PASS || echo FAIL
```
**Failure signal:** Exit 2 (ambiguous — prefix matches multiple), exit 3 (not found), or missing expected fields.
**Notes:** Tests the `find_session_ids_by_prefix` function. If the prefix is ambiguous (matches 2+ sessions), exit code 2 is correct behavior, not a bug.

---

## T-07: show latest resolves to most recent session
**Level:** Intermediate
**Verb:** `show`
**What it tests:** The `latest` keyword resolves to `ORDER BY started_at DESC LIMIT 1`
**Command:**
```bash
gaal show latest
```
**Expected:** Exit code 0. JSON object. The `id` field matches the first result of `gaal ls --limit 1`.
```bash
LATEST_LS=$(gaal ls --limit 1 | jq -r '.[0].id')
LATEST_SHOW=$(gaal show latest | jq -r '.id')
[ "$LATEST_LS" = "$LATEST_SHOW" ] && echo PASS || echo "FAIL: ls=$LATEST_LS show=$LATEST_SHOW"
```
**Failure signal:** IDs do not match, or `show latest` returns a different session than the most recent.
**Notes:** Both `ls` and `show latest` should agree on recency ordering. A mismatch indicates sort inconsistency between `list_sessions` and `find_latest_session_id`.

---

## T-08: search returns scored results with correct schema
**Level:** Intermediate
**Verb:** `search`
**What it tests:** Tantivy BM25 search, fact-level result granularity, score ordering
**Command:**
```bash
gaal search "cargo" --limit 5
```
**Expected:** Exit code 0. JSON array of search results. Each result has: `session_id`, `engine`, `turn`, `fact_type`, `subject`, `snippet`, `ts`, `score`, `session_headline`. Results sorted by score descending.
```bash
gaal search "cargo" --limit 5 | jq -e '
  length > 0 and length <= 5 and
  all(
    .session_id and .engine and
    (.turn | type) == "number" and
    .fact_type and .snippet and .ts and
    (.score | type) == "number")
' && echo PASS || echo FAIL
```
**Failure signal:** Exit 10 (tantivy index missing), exit 1 (no results for a query that should match), or results missing `score` field.
**Notes:** The query "cargo" should produce results because many Codex sessions ran cargo commands during gaal development. If exit 1, run `gaal index backfill` to rebuild the Tantivy index.

---

## T-09: who wrote with file path target
**Level:** Intermediate
**Verb:** `who`
**What it tests:** Inverted fact query — find sessions that modified a specific file
**Command:**
```bash
gaal who wrote CLAUDE.md --since 30d --limit 5
```
**Expected:** Exit code 0. JSON array of `WhoRow` objects. Each has: `session_id`, `engine`, `ts`, `fact_type` (must be `"file_write"`), `subject` (contains "CLAUDE.md"), `detail`, `session_headline`.
```bash
gaal who wrote CLAUDE.md --since 30d --limit 5 | jq -e '
  length > 0 and
  all(.fact_type == "file_write") and
  all(.subject | test("CLAUDE.md"))
' && echo PASS || echo FAIL
```
**Failure signal:** Exit 1 (no results), or results with wrong `fact_type`, or subject not containing the target path.
**Notes:** Tests the `MatchMode::Subject` path. The `who wrote` verb restricts to `FactType::FileWrite` and matches subject. CLAUDE.md is a frequently edited file.

---

## T-10: who ran with command fragment
**Level:** Intermediate
**Verb:** `who`
**What it tests:** Command fact lookup by detail substring match
**Command:**
```bash
gaal who ran "cargo build" --since 30d --limit 5
```
**Expected:** Exit code 0. JSON array. Each result has `fact_type: "command"`. The `detail` field contains the substring "cargo build" (case-insensitive).
```bash
gaal who ran "cargo build" --since 30d --limit 5 | jq -e '
  length > 0 and
  all(.fact_type == "command")
' && echo PASS || echo FAIL
```
**Failure signal:** Results with `fact_type` other than `"command"`, or no results when cargo build was definitely run.
**Notes:** Tests `MatchMode::Detail` — the `ran` verb searches the `detail` column, not `subject`. This is the correct design: for commands, `detail` holds the full command string.

---

## T-11: who with invalid verb returns exit code 11
**Level:** Intermediate
**Verb:** `who`
**What it tests:** Verb validation and semantic exit codes
**Command:**
```bash
gaal who exploded test.rs 2>/dev/null
echo $?
```
**Expected:** Exit code 11 (parse error). Stderr JSON contains `"exit_code": 11`.
```bash
gaal who exploded test.rs 2>/dev/null
[ $? -eq 11 ] && echo PASS || echo "FAIL: expected exit 11, got $?"
```
**Failure signal:** Exit code 0 (accepted invalid verb), or exit code other than 11.
**Notes:** The valid verbs are: read, wrote, ran, touched, installed, changed, deleted. Anything else must produce `GaalError::ParseError` with exit code 11. Tests `verb_spec()` function.

---

## T-12: ls with time range filters
**Level:** Intermediate
**Verb:** `ls`
**What it tests:** `--since` and `--before` time parsing (relative durations, dates, today keyword)
**Command:**
```bash
gaal ls --since 7d --before today --limit 5
```
**Expected:** Exit code 0. All returned sessions have `started_at` within the last 7 days and before end of today. JSON array, potentially shorter than limit if fewer sessions exist in range.
```bash
gaal ls --since 7d --before today --limit 5 | jq -e '
  length >= 0 and
  all(.started_at >= "2026-02-25")
' && echo PASS || echo FAIL
```
**Failure signal:** Sessions from outside the time range appearing in results, or parse error on `7d` / `today`.
**Notes:** Tests `parse_time_bound` which handles: relative durations (1h, 3d, 2w), keywords (today, yesterday), ISO dates (2026-03-01), and RFC3339. The `before` flag uses upper-bound logic (23:59:59 for dates).

---

## T-13: active discovers running processes with correct schema
**Level:** Advanced
**Verb:** `active`
**What it tests:** Live PID probing, process discovery, JSONL-to-session mapping
**Command:**
```bash
gaal active
```
**Expected:** If agent processes are running: exit code 0, JSON array. Each element has: `id`, `engine`, `pid` (number > 0), `cwd`, `uptime_secs`, `cpu_pct`, `rss_mb`, `status`, `stuck_signals`. If no agents running: exit code 1.
```bash
gaal active 2>/dev/null && {
  gaal active | jq -e '
    length > 0 and
    all(
      .id and .engine and
      (.pid | type) == "number" and .pid > 0 and
      .cwd and
      (.uptime_secs | type) == "number" and
      .stuck_signals
    )
  ' && echo PASS || echo FAIL
} || echo "SKIP: no active sessions (exit 1 is expected)"
```
**Failure signal:** Missing `pid` field, `pid` of 0, or `status` not one of: active/idle/stuck.
**Notes:** Requires running Claude or Codex processes. The `find_active_sessions` function uses `pgrep -x claude`, `pgrep -x codex`, and `ps aux` fallback. On macOS, CWD resolution uses `lsof -p PID -Ffn`.

---

## T-14: inspect on a completed (archived) session
**Level:** Advanced
**Verb:** `inspect`
**What it tests:** Archived session inspection — no PID, process is null, last_turn instead of current_turn
**Command:**
```bash
SESSION_ID=$(gaal ls --status completed --limit 1 | jq -r '.[0].id')
gaal inspect "$SESSION_ID"
```
**Expected:** Exit code 0. JSON object with: `pid: null`, `process: null`, `status` is "completed" or "failed", `current_turn: null` (absent via skip_serializing_if), `velocity` present, `stuck_signals` present, `context` present.
```bash
SESSION_ID=$(gaal ls --status completed --limit 1 | jq -r '.[0].id')
gaal inspect "$SESSION_ID" | jq -e '
  .id and
  .pid == null and
  .process == null and
  (.status == "completed" or .status == "failed") and
  .context and
  .context.tokens_used >= 0 and
  .stuck_signals
' && echo PASS || echo FAIL
```
**Failure signal:** Exit 3 (not found — inspect cannot find archived sessions), or `pid` is non-null for a completed session.
**Notes:** Tests the `inspect_archived` code path. For completed sessions, the runtime probe reads the JSONL file directly (if it still exists) to populate velocity/errors.

---

## T-15: inspect --active shows all running sessions
**Level:** Advanced
**Verb:** `inspect`
**What it tests:** Batch live inspection mode
**Command:**
```bash
gaal inspect --active
```
**Expected:** If agents running: exit code 0, JSON array of InspectOutput objects. Each has `pid` (non-null), `process` (non-null with `cpu_pct` and `rss_mb`), `status` one of active/idle/stuck, `velocity`, `stuck_signals`. If no agents: exit code 1.
```bash
gaal inspect --active 2>/dev/null && {
  gaal inspect --active | jq -e '
    type == "array" and length > 0 and
    all(.pid != null and .process != null and .process.cpu_pct >= 0)
  ' && echo PASS || echo FAIL
} || echo "SKIP: no active sessions"
```
**Failure signal:** Returns single object instead of array, or `process` is null for a live session.
**Notes:** Tests the `InspectPayload::Many` variant. The `--active` flag iterates `find_active_sessions()` and produces an inspect snapshot for each.

---

## T-16: handoff requires agent-mux (graceful failure without it)
**Level:** Advanced
**Verb:** `handoff`
**What it tests:** LLM dispatch pipeline setup, error handling when agent-mux is unavailable
**Command:**
```bash
SESSION_ID=$(gaal ls --status completed --limit 1 | jq -r '.[0].id')
gaal handoff "$SESSION_ID" 2>/dev/null
echo "exit: $?"
```
**Expected:** If agent-mux is installed and configured: exit code 0, JSON array of `HandoffRunResult` objects with `session_id`, `handoff_path`, `headline`, `projects`, `keywords`, `substance`. If agent-mux is NOT installed: exit code 1 (IO error — spawn failed). The error message should indicate agent-mux failure, not a crash or panic.
```bash
SESSION_ID=$(gaal ls --status completed --limit 1 | jq -r '.[0].id')
OUTPUT=$(gaal handoff "$SESSION_ID" 2>&1)
EXIT=$?
if [ $EXIT -eq 0 ]; then
  echo "$OUTPUT" | jq -e '
    length > 0 and all(.session_id and .handoff_path)
  ' && echo "PASS: handoff generated" || echo "FAIL: bad output shape"
else
  echo "$OUTPUT" | grep -qi "agent-mux\|spawn\|No such file" && echo "PASS: graceful failure (exit $EXIT)" || echo "FAIL: unexpected error"
fi
```
**Failure signal:** Panic/crash, or exit code that does not indicate the agent-mux dependency issue.
**Notes:** The `invoke_agent_mux` function shells out to the configured binary. This test validates the full pipeline: facts gathering, context building, LLM dispatch, response parsing, handoff MD writing, and DB upsert.

---

## T-17: recall with no handoffs returns exit code 1
**Level:** Advanced
**Verb:** `recall`
**What it tests:** Graceful degradation when handoffs table is empty
**Command:**
```bash
gaal recall "gaal" 2>/dev/null
echo "exit: $?"
```
**Expected:** Exit code 1 (NoResults). With 0 handoffs in the index, recall has nothing to score and must return NoResults rather than crash.
```bash
HANDOFFS=$(gaal index status | jq '.handoffs_total')
gaal recall "gaal" 2>/dev/null
EXIT=$?
if [ "$HANDOFFS" -eq 0 ]; then
  [ $EXIT -eq 1 ] && echo "PASS: correct NoResults exit with 0 handoffs" || echo "FAIL: expected exit 1 with 0 handoffs, got $EXIT"
else
  [ $EXIT -eq 0 ] && echo "PASS: recall returned results" || echo "FAIL: recall failed with $HANDOFFS handoffs"
fi
```
**Failure signal:** Panic, exit 10, or exit 11 instead of clean exit 1. Also: if handoffs exist but recall still returns exit 1, the scoring algorithm has a bug.
**Notes:** **KNOWN BUG**: When `handoffs_total` is 0, `recall` correctly returns exit 1, but the design specifies a fallback to "most recent substantive sessions." Since no handoffs exist, there are no substance scores, so the fallback also produces nothing. After `gaal handoff today` populates handoffs, recall should start working. This test documents the current behavior.

---

## T-18: Epoch-dated eywa stubs pollute oldest_session
**Level:** Stress
**Verb:** `index status`
**What it tests:** Data integrity — eywa import stubs use epoch timestamp
**Command:**
```bash
gaal index status | jq -r '.oldest_session'
```
**Expected (current — KNOWN BUG):** Returns `"1970-01-01T00:00:00Z"` because `build_eywa_session_stub` uses `EPOCH_RFC3339` as a fallback `started_at` when the eywa entry has no date. This pollutes `MIN(started_at)`.
```bash
OLDEST=$(gaal index status | jq -r '.oldest_session')
if [ "$OLDEST" = "1970-01-01T00:00:00Z" ]; then
  echo "KNOWN BUG CONFIRMED: oldest_session is epoch (eywa stub pollution)"
else
  echo "FIXED: oldest_session is $OLDEST"
fi
```
**Failure signal:** N/A — this test confirms the known bug exists. When fixed, `oldest_session` should be a real date (e.g., `2026-01-*` or later).
**Notes:** **KNOWN BUG #1.** The fix should either: (a) exclude epoch-dated sessions from the MIN query, (b) set a sensible default date during eywa import (e.g., the generated_at timestamp), or (c) skip sessions with `started_at < '2000-01-01'` in status computation. Affects `get_index_status` in `db/queries.rs` line 777.

---

## T-19: Active sessions duplicate same ID across multiple PIDs
**Level:** Stress
**Verb:** `active`
**What it tests:** Process deduplication — same session ID reported once or deduplicated
**Command:**
```bash
gaal active 2>/dev/null | jq '[.[].id] | group_by(.) | map(select(length > 1)) | length'
```
**Expected (current — KNOWN BUG):** Returns a number > 0, because `find_active_sessions` discovers multiple PIDs (parent + subagent child processes) that all resolve to the same session ID, and `gaal active` does not deduplicate.
```bash
gaal active 2>/dev/null && {
  DUPES=$(gaal active | jq '[.[].id] | group_by(.) | map(select(length > 1)) | length')
  if [ "$DUPES" -gt 0 ]; then
    echo "KNOWN BUG CONFIRMED: $DUPES session IDs appear multiple times in active output"
    gaal active | jq '[.[].id] | group_by(.) | map({id: .[0], count: length}) | sort_by(-.count)'
  else
    echo "FIXED: all active session IDs are unique"
  fi
} || echo "SKIP: no active sessions"
```
**Failure signal:** N/A — this test confirms the known bug. When fixed, each session ID should appear at most once, with process info aggregated or the primary PID selected.
**Notes:** **KNOWN BUG #2.** When Claude Code spawns subagent processes, multiple PIDs share the same CWD and JSONL path. Each PID is independently discovered by `list_agent_processes()` and each maps to the same session ID via `extract_session_id`. The fix should deduplicate by session ID, keeping the PID with the highest CPU usage or the one that is the actual parent process.

---

## T-20: Claude session token counts are near-zero
**Level:** Stress
**Verb:** `ls` / `show`
**What it tests:** Claude JSONL parser token accumulation
**Command:**
```bash
gaal ls --engine claude --limit 5 | jq '[.[] | .tokens.input + .tokens.output] | map(select(. < 100)) | length'
```
**Expected (current — KNOWN BUG):** Returns a number > 0, because many Claude sessions show token counts in single digits (e.g., input: 13, output: 11) when the actual session consumed hundreds of thousands of tokens.
```bash
gaal ls --engine claude --limit 10 | jq '
  [.[] | select(.tools_used > 5) | {id: .id, tools: .tools_used, total_tokens: (.tokens.input + .tokens.output)}]
  | map(select(.total_tokens < 1000))
' > /tmp/gaal-token-bug.json
COUNT=$(jq 'length' /tmp/gaal-token-bug.json)
if [ "$COUNT" -gt 0 ]; then
  echo "KNOWN BUG CONFIRMED: $COUNT Claude sessions with >5 tool calls but <1000 total tokens"
  jq '.[0]' /tmp/gaal-token-bug.json
else
  echo "FIXED: all active Claude sessions have plausible token counts"
fi
```
**Failure signal:** N/A — this test confirms the known bug. When fixed, a Claude session with 5+ tool calls should have at least tens of thousands of tokens.
**Notes:** **KNOWN BUG #3.** The Claude JSONL parser (`parser/claude.rs`) is not correctly summing `usage.input_tokens` and `usage.output_tokens` from assistant message records. The Codex parser does not have this issue — Codex sessions show millions of tokens as expected. The fix is in `claude::parse()` where token accumulation from `message.usage` fields needs to be implemented or corrected. Compare with the working Codex parser logic.

---

## T-BONUS-04: Status "active" count includes stale sessions without ended_at
**Level:** Stress
**Verb:** `index status`
**What it tests:** Whether sessions_by_status "active" count is inflated
**Command:**
```bash
gaal index status | jq '.sessions_by_status.active // 0'
```
**Expected (current — KNOWN BUG):** Returns a number significantly larger than the actual count of running processes. For example, `sessions_by_status.active: 35` when only 2-3 agent processes are actually alive.
```bash
ACTIVE_IN_INDEX=$(gaal index status | jq '.sessions_by_status.active // 0')
ACTUAL_PROCS=$(gaal active 2>/dev/null | jq 'length' 2>/dev/null || echo 0)
echo "Index says $ACTIVE_IN_INDEX active, actual running: $ACTUAL_PROCS"
if [ "$ACTIVE_IN_INDEX" -gt 10 ] && [ "$ACTUAL_PROCS" -lt 10 ]; then
  echo "KNOWN BUG CONFIRMED: $((ACTIVE_IN_INDEX - ACTUAL_PROCS)) stale 'active' sessions in index"
else
  echo "OK or FIXED"
fi
```
**Failure signal:** N/A — this test confirms the known bug.
**Notes:** **KNOWN BUG #4.** The `session_status_from_fields` function in `db/queries.rs` classifies any session with `ended_at IS NULL` as "active". But many sessions have `ended_at = NULL` simply because the JSONL file did not have a clean session-end marker, or the parser did not extract it. The fix should cross-reference with live PID probing, or mark sessions as "unknown" if `ended_at IS NULL` and `last_event_at` is old.

---

## T-BONUS-05: show --files write suppresses other fact sections
**Level:** Integration
**Verb:** `show`
**What it tests:** Focused view filtering — when a fact filter flag is active, other sections are omitted from JSON
**Command:**
```bash
gaal show latest --files write
```
**Expected:** Exit code 0. JSON object contains `files` key. Does NOT contain `commands`, `errors`, or `git_ops` keys (they are stripped by `to_json_value` when `any_fact_filter` is true).
```bash
gaal show latest --files write | jq -e '
  has("files") and
  (has("commands") | not) and
  (has("errors") | not) and
  (has("git_ops") | not)
' && echo PASS || echo FAIL
```
**Failure signal:** Output contains `commands`, `errors`, or `git_ops` alongside `files` when only `--files write` was requested.
**Notes:** Tests the focused view logic in `show.rs` where `any_fact_filter` is set when specific view flags are passed. The design principle is progressive disclosure — only return what was asked for. **KNOWN BUG #5**: This currently works correctly for `--files`, `--commands`, `--errors`, and `--git` individually, but when NO fact filter is specified (bare `gaal show`), ALL sections are included. The question is whether `--files write` properly excludes unrelated sections. This test verifies the correct behavior.

# TESTS-E2E.md — Gaal Agentic Usage Tests

Tracked: 2026-03-11

These are **agentic workflow tests** — they simulate how a real AI agent chains gaal commands together to solve a problem. Each test runs against live data on this machine and includes decision points where the agent makes choices based on output.

---

## Test Matrix

| # | Name | Difficulty | Chain Length | Validates |
|---|------|-----------|-------------|-----------|
| 1 | Session Continuity Cold Start | medium | 4 | recall, show, decision routing |
| 2 | Fleet Awareness Coordinator | medium | 4 | active, inspect, ls, fleet triage |
| 3 | File Attribution Trace | medium | 4 | who wrote, show, search, provenance chain |
| 4 | Salt Self-Identification | hard | 3 (separate calls) | salt, find-salt, JSONL flush protocol |
| 5 | Cross-Session Topic Search | medium | 4 | search, recall, show, relevance ranking |
| 6 | File Conflict Detection | hard | 5 | who wrote, show, cross-session overlap |
| 7 | Debugging Chain | hard | 5 | show --errors, search, show --trace, error attribution |
| 8 | Session Health Triage | medium | 3 | inspect, ls, velocity/health signals |
| 9 | Deep Knowledge Retrieval | hard | 5 | recall, search, show, multi-hop retrieval |
| 10 | Full Pipeline: Index to Handoff | hard | 6 | index status, backfill, ls, show, search, recall |

---

## Test 1: Session Continuity Cold Start

**Scenario:** An agent starts fresh and needs to resume work on a specific topic. It doesn't know which session to continue from — it only knows the topic name. It must find the most relevant prior session, extract actionable context, and decide whether the prior work is recent enough to build on.

**Steps:**

```bash
# 1. Recall sessions about the topic
OUTPUT_1=$(gaal recall "gaal session detection" --format brief --limit 3)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse `OUTPUT_1`. Extract `session` IDs and `score` values. If the top result has `score > 10`, it's highly relevant — proceed to step 2 with that session ID. If `score < 5`, the topic is unfamiliar — FAIL the test (gaal has extensive session detection history, so this should score high).

```bash
# 2. Get the top session's full details
SESSION_ID=$(echo "$OUTPUT_1" | head -1 | grep -oP 'session: \K\w+')
OUTPUT_2=$(gaal show "$SESSION_ID")
echo "$OUTPUT_2"
```

**Decision point 2:** Parse `OUTPUT_2` JSON. Check `duration_secs` and `tools_used`. If `tools_used > 5`, this was a substantive session — proceed. If `tools_used <= 5`, it was trivial — go back to step 1 and try the next session ID.

```bash
# 3. Check what files this session modified
OUTPUT_3=$(gaal show "$SESSION_ID" --files write)
echo "$OUTPUT_3"
```

```bash
# 4. Confirm the session is indexed and searchable
OUTPUT_4=$(gaal search "session detection" --limit 1)
echo "$OUTPUT_4"
```

**Pass criteria:**
- Step 1 returns at least 1 result with `score > 5`
- Step 2 returns valid JSON with `id`, `engine`, `duration_secs`, `tools_used` fields
- Step 3 returns valid JSON with `files` object containing `written`, `edited`, `read` arrays
- Step 4 returns at least 1 search result with `session_id` and `score` fields
- The agent successfully identified a substantive prior session and could extract enough context to continue

**What it validates:** `recall` ranking quality, `show` detail completeness, `show --files` output shape, `search` consistency with recall results

---

## Test 2: Fleet Awareness Coordinator

**Scenario:** A coordinator agent needs to understand the current fleet state before dispatching new workers. It needs to know: what's running, are any sessions idle/stuck, and what recently completed. This is the exact check a coordinator runs at the start of every dispatch cycle.

**Steps:**

```bash
# 1. What's running right now?
OUTPUT_1=$(gaal active)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse `OUTPUT_1` JSON array. Count sessions. If `length == 0`, skip to step 3 (no active fleet). If `length > 0`, extract session with highest `uptime_secs` — that's likely the coordinator itself.

```bash
# 2. Inspect the longest-running active session
LONGEST_ID=$(echo "$OUTPUT_1" | jq -r 'sort_by(-.uptime_secs) | .[0].id' | cut -c1-8)
OUTPUT_2=$(gaal inspect "$LONGEST_ID")
echo "$OUTPUT_2"
```

**Decision point 2:** Parse `OUTPUT_2`. Check `velocity.actions_per_minute_5m`. If `> 0`, session is working. If `== 0` and `uptime_secs > 3600`, session is idle — flag it.

```bash
# 3. What completed recently?
OUTPUT_3=$(gaal ls --since 1d --limit 10)
echo "$OUTPUT_3"
```

**Decision point 3:** Parse `OUTPUT_3` JSON array. Count sessions by `engine`. This tells the coordinator the recent engine mix and whether to dispatch claude or codex workers.

```bash
# 4. Cross-reference: any active session also in recent ls?
# (validates that active detection and index are consistent)
```

**Pass criteria:**
- Step 1 returns valid JSON array (may be empty `[]` if nothing running — that's OK)
- Step 2 returns valid JSON with `velocity` and `context` objects (skip if step 1 was empty)
- Step 3 returns valid JSON array with at least 1 session, each having `id`, `engine`, `status`, `duration_secs`
- `active` sessions that appear in `ls` must have matching `engine` and compatible `cwd`
- No command returns exit code > 1

**What it validates:** `active` process discovery, `inspect` operational snapshot, `ls` fleet view, cross-command consistency

---

## Test 3: File Attribution Trace

**Scenario:** An agent encounters a file that seems wrong and needs to trace who last modified it and why. This is the file forensics workflow — start from a filename, find the sessions that touched it, drill into the most recent one, and extract the rationale.

**Steps:**

```bash
# 1. Who wrote this file recently?
OUTPUT_1=$(gaal who wrote "CLAUDE.md" --since 30d)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse `OUTPUT_1`. It should have `sessions` array. If `total > 0`, pick the session with the most recent `latest_ts`. If `total == 0`, the file hasn't been modified by any tracked session — test still passes if exit code is 1 (no results).

```bash
# 2. Drill into the most recent writer
WRITER_ID=$(echo "$OUTPUT_1" | jq -r '.sessions[0].session_id')
OUTPUT_2=$(gaal show "$WRITER_ID")
echo "$OUTPUT_2"
```

**Decision point 2:** Check `OUTPUT_2`. Verify `engine` matches what `who` reported. Check `duration_secs > 0`.

```bash
# 3. Search for context around what that session was doing
OUTPUT_3=$(gaal search "CLAUDE.md" --limit 3)
echo "$OUTPUT_3"
```

```bash
# 4. Check if other sessions also wrote this file (conflict signal)
WRITER_COUNT=$(echo "$OUTPUT_1" | jq '.total')
echo "Total writers in window: $WRITER_COUNT"
```

**Decision point 3:** If `WRITER_COUNT > 3`, multiple sessions modified this file — potential coordination issue. If `WRITER_COUNT == 1`, clean single-owner pattern.

**Pass criteria:**
- Step 1 returns JSON with `sessions` array and `total` count, `search_window` string
- Step 2 returns valid session JSON with `engine` field matching step 1's `engine`
- Step 3 returns search results referencing the same file
- Engine values are always one of: `claude`, `codex`
- All session IDs are exactly 8 hex characters

**What it validates:** `who wrote` inverted query, `show` session detail, `search` content matching, session ID format consistency

---

## Test 4: Salt Self-Identification

**Scenario:** A session needs to find its own JSONL file for self-handoff. This is the most trust-critical workflow — if salt/find-salt doesn't work, sessions can't generate their own handoffs. The test validates the entire content-addressed self-identification protocol.

**CRITICAL: Steps 1 and 2 MUST be separate Bash tool invocations. The JSONL flush happens between calls.**

**Step 1** (separate Bash call):

```bash
# Generate and embed salt
SALT=$(gaal salt)
echo "$SALT"
```

**Validation:** Output must match pattern `GAAL_SALT_[0-9a-f]{16}`. Store the full token.

**Step 2** (separate Bash call — MUST NOT be chained with step 1):

```bash
# Find own JSONL using the salt from step 1
SALT="<salt-from-step-1>"
OUTPUT_2=$(gaal find-salt "$SALT")
echo "$OUTPUT_2"
```

**Decision point:** Parse `OUTPUT_2`. If it returns JSON with `jsonl_path`, `engine`, and `session_id` — the salt protocol works. If it returns an error or empty result, the JSONL hasn't flushed — FAIL.

**Step 3** (separate Bash call):

```bash
# Verify the discovered JSONL actually exists
JSONL_PATH=$(echo "$OUTPUT_2" | jq -r '.jsonl_path')
ls -la "$JSONL_PATH"
```

**Pass criteria:**
- Step 1 output matches `GAAL_SALT_[0-9a-f]{16}`
- Step 2 returns valid JSON with `jsonl_path` (absolute path), `engine` (claude or codex), `session_id` (8 hex chars)
- Step 3 confirms the JSONL file exists on disk
- The `session_id` returned should match this session's actual ID
- Total elapsed between step 1 and step 2 < 30 seconds (flush should be fast)

**What it validates:** `salt` token generation, JSONL flush timing, `find-salt` content-addressed search, file path resolution, self-identification protocol integrity

---

## Test 5: Cross-Session Topic Search

**Scenario:** An agent needs to find past discussions about a specific technical topic that might be spread across multiple sessions, engines, and time ranges. This tests gaal's ability to surface relevant content across the full corpus.

**Steps:**

```bash
# 1. BM25 search for the topic
OUTPUT_1=$(gaal search "handoff extraction" --limit 5)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse results. If multiple `session_id` values appear, the topic spans sessions — good. Note the `fact_type` distribution (command, error, file_read, file_write).

```bash
# 2. Semantic recall for the same topic
OUTPUT_2=$(gaal recall "handoff extraction" --format brief --limit 3)
echo "$OUTPUT_2"
```

**Decision point 2:** Compare session IDs from search vs recall. They use different ranking algorithms (BM25 vs IDF+recency). The top results may differ — that's expected and healthy. If recall returns zero results but search found many, the handoff index may be stale.

```bash
# 3. Drill into the highest-scored search result
TOP_SESSION=$(echo "$OUTPUT_1" | jq -r '.[0].session_id')
OUTPUT_3=$(gaal show "$TOP_SESSION")
echo "$OUTPUT_3"
```

```bash
# 4. Widen the search window if results are sparse
OUTPUT_4=$(gaal recall "handoff extraction" --days-back 60 --format brief --limit 5)
echo "$OUTPUT_4"
```

**Decision point 3:** Compare result counts between default window (step 2) and 60-day window (step 4). If the 60-day window surfaces significantly more results, recent work on this topic is sparse.

**Pass criteria:**
- Step 1 returns JSON array with at least 1 result, each having `session_id`, `score`, `fact_type`, `snippet`
- Step 2 returns array of brief-format strings (not JSON objects — recall brief format is newline-delimited text)
- Step 3 returns valid session JSON for the drilled-in session
- Step 4 returns results (may be same as step 2 if all sessions are within default window)
- No exit code > 1 on any step

**What it validates:** `search` BM25 ranking, `recall` IDF+recency ranking, cross-algorithm result comparison, `--days-back` window expansion, search-to-show pipeline

---

## Test 6: File Conflict Detection

**Scenario:** A coordinator suspects two workers may have modified the same file concurrently. It needs to identify overlapping file writes across sessions within a time window and determine whether the modifications conflict.

**Steps:**

```bash
# 1. Find all sessions that wrote to a common file
OUTPUT_1=$(gaal who wrote "MEMORY.md" --since 14d)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse `sessions` array. If `total > 1`, multiple sessions wrote this file — potential conflict. Extract all session IDs.

```bash
# 2. Get details on each writing session to check time overlap
SESSION_IDS=$(echo "$OUTPUT_1" | jq -r '.sessions[].session_id' | head -3)
for SID in $SESSION_IDS; do
  gaal show "$SID" | jq '{id: .id, engine: .engine, started: .started_at, ended: .ended_at, cwd: .cwd}'
done
```

**Decision point 2:** Check for time overlap — if session A's `started_at` < session B's `ended_at` AND session B's `started_at` < session A's `ended_at`, they overlapped temporally. If they also share the same `cwd`, this is a true conflict.

```bash
# 3. Check if these sessions touched other common files
for SID in $SESSION_IDS; do
  echo "--- $SID ---"
  gaal show "$SID" --files write | jq '.files'
done
```

```bash
# 4. Search for any session mentioning the file to find broader context
OUTPUT_4=$(gaal search "MEMORY.md" --limit 5)
echo "$OUTPUT_4"
```

```bash
# 5. Cross-reference: does recall surface these sessions?
OUTPUT_5=$(gaal recall "MEMORY.md changes" --format brief --limit 3)
echo "$OUTPUT_5"
```

**Pass criteria:**
- Step 1 returns JSON with `sessions` array, each having `session_id`, `engine`, `latest_ts`, `fact_count`
- Step 2 returns valid JSON for each session with non-null `started_at` and `ended_at`
- Step 3 returns `files` object for each session (may have empty arrays — that's valid)
- All session IDs cross-resolve correctly between `who` output and `show` input
- If overlap detected: both sessions' engines and CWDs are reported accurately

**What it validates:** `who wrote` multi-session results, `show` batch resolution, temporal overlap analysis from session metadata, `--files write` output, cross-command ID consistency

---

## Test 7: Debugging Chain

**Scenario:** Something broke in a recent session. The agent needs to trace back through session history to find which session introduced the failure, what error occurred, and what was being attempted. This is the forensic debugging workflow.

**Steps:**

```bash
# 1. List recent sessions and find one with errors
OUTPUT_1=$(gaal ls --since 3d --limit 20)
echo "$OUTPUT_1" | jq '[.[] | select(.status == "completed")] | length'
```

**Decision point 1:** Parse the list. We need to find a session that had errors. Pick a session with high `tools_used` count (more likely to have hit errors).

```bash
# 2. Check errors in a busy session
BUSY_ID=$(echo "$OUTPUT_1" | jq -r '[.[] | select(.tools_used > 10)] | sort_by(-.tools_used) | .[0].id')
OUTPUT_2=$(gaal show "$BUSY_ID" --errors)
echo "$OUTPUT_2" | jq '.errors | length'
```

**Decision point 2:** If `errors | length > 0`, we found our debugging target. Extract the first error's `cmd` and `snippet`. If no errors, pick the next session.

```bash
# 3. Search for the error pattern across all sessions
ERROR_CMD=$(echo "$OUTPUT_2" | jq -r '.errors[0].cmd // empty' | head -c 50)
if [ -n "$ERROR_CMD" ]; then
  OUTPUT_3=$(gaal search "$ERROR_CMD" --limit 5)
  echo "$OUTPUT_3"
fi
```

**Decision point 3:** If search returns results from other sessions with the same error pattern, this is a recurring issue. If only one session, it's isolated.

```bash
# 4. Inspect the session's operational state
OUTPUT_4=$(gaal inspect "$BUSY_ID")
echo "$OUTPUT_4" | jq '{velocity: .velocity, recent_errors: (.recent_errors | length), context: .context}'
```

```bash
# 5. Check what the session was working on right before the error
OUTPUT_5=$(gaal show "$BUSY_ID")
echo "$OUTPUT_5" | jq '{cwd: .cwd, model: .model, tools: .tools_used, duration: .duration_secs}'
```

**Pass criteria:**
- Step 1 returns JSON array of sessions from the last 3 days
- Step 2 returns session JSON with `errors` array (may be empty — test adapts)
- Step 3 returns search results if an error pattern was found
- Step 4 returns inspect JSON with `velocity` object and `recent_errors` array
- Step 5 provides session context (cwd, model) to explain what was being attempted
- No gaal command itself errors (exit code 0 on all steps)

**What it validates:** `ls` time filtering, `show --errors` error extraction, `search` for error patterns, `inspect` operational snapshot, multi-step forensic reasoning

---

## Test 8: Session Health Triage

**Scenario:** A coordinator needs to quickly assess whether its active workers are productive or spinning. It checks velocity, token consumption, and recent error rates across all active sessions to decide whether to intervene.

**Steps:**

```bash
# 1. Get all active sessions
OUTPUT_1=$(gaal active)
ACTIVE_COUNT=$(echo "$OUTPUT_1" | jq 'length')
echo "Active sessions: $ACTIVE_COUNT"
```

**Decision point 1:** If `ACTIVE_COUNT == 0`, fallback to checking the 3 most recent completed sessions instead (the fleet is quiescent). If `> 0`, proceed to inspect each.

```bash
# 2. Inspect active sessions (or recent completed if none active)
if [ "$ACTIVE_COUNT" -gt 0 ]; then
  # Inspect the session with highest CPU
  TARGET=$(echo "$OUTPUT_1" | jq -r 'sort_by(-.cpu_pct) | .[0].id' | cut -c1-8)
else
  TARGET=$(gaal ls --limit 1 | jq -r '.[0].id')
fi
OUTPUT_2=$(gaal inspect "$TARGET")
echo "$OUTPUT_2"
```

**Decision point 2:** Parse `velocity.actions_per_minute_5m`:
- `> 1.0`: healthy, productive
- `0.1 - 1.0`: slow but working
- `== 0.0` with `uptime_secs > 300`: possibly stuck or idle

```bash
# 3. Check recent errors for this session
OUTPUT_3=$(echo "$OUTPUT_2" | jq '{errors: .recent_errors | length, velocity: .velocity.actions_per_minute_5m, tokens_per_min: .velocity.tokens_per_minute_5m}')
echo "$OUTPUT_3"
```

**Pass criteria:**
- Step 1 returns valid JSON array (empty is OK)
- Step 2 returns inspect JSON with `velocity`, `context`, `recent_errors` fields
- Step 3 successfully extracts health metrics
- Velocity value is a number >= 0
- Token count is a number >= 0
- The agent can make a clear healthy/slow/stuck classification from the data

**What it validates:** `active` fleet enumeration, `inspect` health signals, velocity-based triage logic, graceful fallback when fleet is empty

---

## Test 9: Deep Knowledge Retrieval

**Scenario:** An agent needs context from a past session about a rare topic — not the most recent session, but one from weeks ago. The agent must use recall to find candidates, verify relevance by drilling in, and potentially chain through related sessions to build full context.

**Steps:**

```bash
# 1. Broad recall with extended window
OUTPUT_1=$(gaal recall "eywa migration" --days-back 60 --format brief --limit 5)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse results. Extract session IDs and scores. If no results, try alternative queries: "eywa replacement", "eywa handoff", "session observability migration". This tests the agent's ability to reformulate queries.

```bash
# 2. Try an alternative query if first was sparse
OUTPUT_2=$(gaal recall "eywa replacement" --days-back 60 --format brief --limit 5)
echo "$OUTPUT_2"
```

**Decision point 2:** Compare result sets. Merge unique session IDs from both queries to build a broader picture.

```bash
# 3. Drill into the most relevant session
BEST_SESSION=$(echo "$OUTPUT_1" | head -1 | grep -oP 'session: \K\w+')
OUTPUT_3=$(gaal show "$BEST_SESSION")
echo "$OUTPUT_3"
```

```bash
# 4. Search for specific artifacts from that session
CWD=$(echo "$OUTPUT_3" | jq -r '.cwd')
OUTPUT_4=$(gaal search "eywa" --limit 5)
echo "$OUTPUT_4"
```

```bash
# 5. Follow the trail — check what other sessions touched the same directory
OUTPUT_5=$(gaal who touched "eywa" --since 60d)
echo "$OUTPUT_5"
```

**Pass criteria:**
- Step 1 or step 2 returns at least 1 result (the eywa→gaal migration is well-documented in session history)
- Step 3 returns valid session JSON for the drilled-in session
- Step 4 returns search results mentioning "eywa"
- Step 5 returns sessions that touched eywa-related files/commands
- The agent can trace from a vague topic query through to specific session artifacts
- Multi-hop: session ID from recall resolves correctly in show, and CWD from show is valid

**What it validates:** `recall` with extended `--days-back`, query reformulation, `show` deep drill, `search` corpus coverage, `who touched` broad matching, multi-hop resolution chain

---

## Test 10: Full Pipeline — Index to Handoff Readiness

**Scenario:** Validate the complete gaal data pipeline from raw JSONL to queryable index to retrievable recall. This is the infrastructure health check — if any stage is broken, downstream features degrade silently.

**Steps:**

```bash
# 1. Check index health
OUTPUT_1=$(gaal index status)
echo "$OUTPUT_1"
```

**Decision point 1:** Parse JSON. Verify:
- `sessions_total > 0` (index is not empty)
- `handoffs_total > 0` (handoffs exist for recall)
- `sessions_by_engine` has both `claude` and `codex` keys
- `last_indexed_at` is within the last 24 hours

```bash
# 2. Run a backfill to ensure freshness (idempotent, safe)
OUTPUT_2=$(gaal index backfill --since 1d)
echo "$OUTPUT_2"
```

**Decision point 2:** Backfill should report `indexed`, `skipped`, `errors` counts. If `errors > 0`, something is wrong with recent JSONL files.

```bash
# 3. Verify a recent session is queryable
RECENT_ID=$(gaal ls --limit 1 | jq -r '.[0].id')
OUTPUT_3=$(gaal show "$RECENT_ID")
echo "$OUTPUT_3" | jq '{id: .id, engine: .engine, status: .status}'
```

```bash
# 4. Verify search returns results for a known term
OUTPUT_4=$(gaal search "coordinator" --limit 3)
echo "$OUTPUT_4" | jq 'length'
```

**Decision point 3:** If search returns 0 results for "coordinator" (a ubiquitous term), Tantivy index is broken.

```bash
# 5. Verify recall returns results
OUTPUT_5=$(gaal recall --format brief --limit 3)
echo "$OUTPUT_5"
```

**Decision point 4:** If recall returns results but search didn't (or vice versa), there's a desync between SQLite and Tantivy.

```bash
# 6. Verify session ID cross-resolution
# Take a session from search results and resolve it via show
SEARCH_SESSION=$(echo "$OUTPUT_4" | jq -r '.[0].session_id')
OUTPUT_6=$(gaal show "$SEARCH_SESSION")
echo "$OUTPUT_6" | jq '{id: .id, engine: .engine}'
```

**Pass criteria:**
- Step 1: `sessions_total > 100`, `handoffs_total > 50`, both engines present
- Step 2: `errors == 0`
- Step 3: Returns valid session JSON, `id` matches `RECENT_ID`
- Step 4: Returns at least 1 result (3 expected for "coordinator")
- Step 5: Returns at least 1 recall result
- Step 6: Session from search resolves via show — `id` matches `SEARCH_SESSION`
- All exit codes are 0
- No command takes > 10 seconds (performance gate)

**What it validates:** `index status` health reporting, `index backfill` idempotency, `ls` → `show` ID resolution, `search` Tantivy integration, `recall` handoff retrieval, cross-command session ID consistency, full pipeline integrity

---

## Running These Tests

Each test is designed to be copy-pasted into a subagent prompt. The subagent should:

1. Run each step as a separate Bash tool call (especially test 4 — salt protocol)
2. Parse JSON outputs with `jq`
3. Make decisions at each decision point based on actual output
4. Report PASS/FAIL per step and overall
5. Record actual values for key assertions (not just "it returned something")

**Exit code reference:**
- 0 = success
- 1 = no results (valid for `who`, `search` when nothing matches)
- 2 = ambiguous session ID prefix
- 3 = session not found
- 10 = no index
- 11 = parse error

**Output format reference:**
- All commands: JSON to stdout by default
- `recall --format brief`: Newline-delimited text blocks (not JSON objects)
- `active`: JSON array (may be empty `[]`)
- `who`: JSON object with `sessions` array and `total` count
- `search`: JSON array of result objects
- `ls`: JSON array + footer JSON on separate line
- `index status`: Single JSON object

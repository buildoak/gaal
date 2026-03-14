#!/bin/bash
# gaal v0.1.0 comprehensive test runner — fixed assert helper
set -uo pipefail
PASS=0; FAIL=0; SKIP=0

pass() { echo "PASS: $1"; ((PASS++)); }
fail() { echo "FAIL: $1 — $2"; ((FAIL++)); }
skip() { echo "SKIP: $1"; ((SKIP++)); }

# Get session IDs
ID=$(gaal ls --limit 1 2>/dev/null | jq -r '.sessions[0].id')
ID2=$(gaal ls --limit 2 2>/dev/null | jq -r '.sessions[1].id')

if [ -z "$ID" ] || [ "$ID" = "null" ]; then
  echo "FATAL: Cannot get session ID from gaal ls"
  exit 1
fi

echo "=== Suite 1: inspect (9 tests) ==="

# T1: Archived session
OUT=$(gaal inspect "$ID" 2>/dev/null)
echo "$OUT" | jq -e '.id' >/dev/null 2>&1 && pass "T1: has id" || fail "T1: has id" "no .id"
echo "$OUT" | jq -e '.status' >/dev/null 2>&1 && fail "T1: no status" "has status" || pass "T1: no status"
echo "$OUT" | jq -e 'has("process") | not' >/dev/null 2>&1 && pass "T1: no process" || fail "T1: no process" "has process"
BYTES=$(echo "$OUT" | wc -c | tr -d ' ')
[ "$BYTES" -lt 2000 ] && pass "T1: token budget ($BYTES bytes)" || fail "T1: token budget" "$BYTES bytes"

# T2: No args error
gaal inspect 2>/dev/null; [ $? -ne 0 ] && pass "T2: no args error" || fail "T2: no args error" "exited 0"

# T3: Invalid ID
gaal inspect nonexistent_id_xyz 2>/dev/null; [ $? -ne 0 ] && pass "T3: invalid id" || fail "T3: invalid id" "exited 0"

# T4: Files view
OUT=$(gaal inspect "$ID" --files 2>/dev/null)
echo "$OUT" | jq -e '.files' >/dev/null 2>&1 && pass "T4: files present" || fail "T4: files present" "no .files"

# T5: Errors view
OUT=$(gaal inspect "$ID" --errors 2>/dev/null)
echo "$OUT" | jq -e '.errors' >/dev/null 2>&1 && pass "T5: errors present" || fail "T5: errors present" "no .errors"

# T6: Full output
OUT=$(gaal inspect "$ID" -F 2>/dev/null)
echo "$OUT" | jq -e '.commands' >/dev/null 2>&1 && pass "T6: commands array" || fail "T6: commands array" "no .commands"
echo "$OUT" | jq -e '.files' >/dev/null 2>&1 && pass "T6: files array" || fail "T6: files array" "no .files"
echo "$OUT" | jq -e '.errors' >/dev/null 2>&1 && pass "T6: errors array" || fail "T6: errors array" "no .errors"

# T7: Human mode
OUT=$(gaal inspect "$ID" -H 2>/dev/null)
echo "$OUT" | jq . >/dev/null 2>&1 && fail "T7: not JSON" "is JSON" || pass "T7: not JSON"

# T8: Token budget
BYTES=$(gaal inspect "$ID" 2>/dev/null | wc -c | tr -d ' ')
[ "$BYTES" -lt 2000 ] && pass "T8: under budget ($BYTES bytes)" || fail "T8: under budget" "$BYTES bytes"

# T9: Batch mode
OUT=$(gaal inspect --ids "$ID,$ID2" 2>/dev/null)
echo "$OUT" | jq -e 'type == "array" and length == 2' >/dev/null 2>&1 && pass "T9: batch array" || fail "T9: batch array" "not array of 2"

echo ""
echo "=== Suite 2: ls (12 tests) ==="

# T1: Default
OUT=$(gaal ls 2>/dev/null)
echo "$OUT" | jq -e '.sessions | type == "array"' >/dev/null 2>&1 && pass "T1: valid JSON with sessions" || fail "T1: valid JSON with sessions" "no .sessions"
COUNT=$(echo "$OUT" | jq '.sessions | length')
[ "$COUNT" -le 10 ] && pass "T1: <=10 items ($COUNT)" || fail "T1: <=10 items" "$COUNT items"
echo "$OUT" | jq -e '.sessions[0].status' >/dev/null 2>&1 && fail "T1: no status" "has status" || pass "T1: no status"

# T2: Limit
OUT=$(gaal ls --limit 3 2>/dev/null)
COUNT=$(echo "$OUT" | jq '.sessions | length')
[ "$COUNT" -le 3 ] && pass "T2: <=3 items ($COUNT)" || fail "T2: <=3 items" "$COUNT items"

# T3: Engine filter
OUT=$(gaal ls --engine claude 2>/dev/null)
echo "$OUT" | jq -e '.sessions | all(.engine == "claude")' >/dev/null 2>&1 && pass "T3: all claude" || fail "T3: all claude" "mixed engines"

# T4: Since filter + query_window
OUT=$(gaal ls --since 1d 2>/dev/null)
echo "$OUT" | jq -e '.query_window.from' >/dev/null 2>&1 && pass "T4: query_window.from" || fail "T4: query_window.from" "missing"
echo "$OUT" | jq -e '.query_window.to' >/dev/null 2>&1 && pass "T4: query_window.to" || fail "T4: query_window.to" "missing"

# T5: Sort by tokens
OUT=$(gaal ls --sort tokens --limit 5 2>/dev/null)
FIRST=$(echo "$OUT" | jq '.sessions[0].tokens.input')
SECOND=$(echo "$OUT" | jq '.sessions[1].tokens.input')
[ "$FIRST" -ge "$SECOND" ] 2>/dev/null && pass "T5: descending order ($FIRST >= $SECOND)" || fail "T5: descending order" "$FIRST < $SECOND"

# T6: Human mode
OUT=$(gaal ls -H 2>/dev/null)
echo "$OUT" | jq . >/dev/null 2>&1 && fail "T6: not JSON" "is JSON" || pass "T6: not JSON"

# T7: Aggregate
OUT=$(gaal ls --aggregate 2>/dev/null)
echo "$OUT" | jq -e '.total_sessions // .sessions' >/dev/null 2>&1 && pass "T7: aggregate has totals" || fail "T7: aggregate" "no totals"

# T8: Pipe-safe
gaal ls 2>/dev/null | jq '.sessions[0].id' >/dev/null 2>&1 && pass "T8: jq pipe works" || fail "T8: jq pipe" "failed"

# T9: Token budget
BYTES=$(gaal ls 2>/dev/null | wc -c | tr -d ' ')
[ "$BYTES" -lt 5000 ] && pass "T9: under budget ($BYTES bytes)" || fail "T9: under budget" "$BYTES bytes"

# T10: CWD truncation
CWD=$(gaal ls --limit 1 2>/dev/null | jq -r '.sessions[0].cwd')
echo "$CWD" | grep -v '/' >/dev/null && pass "T10: no slashes ($CWD)" || fail "T10: no slashes" "$CWD"

# T11: Query window
OUT=$(gaal ls 2>/dev/null)
echo "$OUT" | jq -e '.query_window.from' >/dev/null 2>&1 && pass "T11: query_window.from" || fail "T11: query_window.from" "missing"
echo "$OUT" | jq -e '.query_window.to' >/dev/null 2>&1 && pass "T11: query_window.to" || fail "T11: query_window.to" "missing"

# T12: Token counts
OUT=$(gaal ls --engine claude --limit 3 2>/dev/null)
TOKENS=$(echo "$OUT" | jq '.sessions[0].tokens.input')
[ "$TOKENS" -gt 100 ] 2>/dev/null && pass "T12: tokens > 100 ($TOKENS)" || fail "T12: tokens > 100" "$TOKENS"

echo ""
echo "=== Suite 3: who (11 tests) ==="

# T1: Wrote
OUT=$(gaal who wrote ISSUES.md 2>/dev/null)
echo "$OUT" | jq -e '.sessions | length > 0' >/dev/null 2>&1 && pass "T1: non-empty" || fail "T1: non-empty" "empty"
echo "$OUT" | jq -e '.sessions[0].fact_count > 0' >/dev/null 2>&1 && pass "T1: fact_count" || fail "T1: fact_count" "zero"

# T2: Read
OUT=$(gaal who read CLAUDE.md 2>/dev/null)
echo "$OUT" | jq -e '.sessions | length > 0' >/dev/null 2>&1 && pass "T2: results" || fail "T2: results" "empty"

# T3: Ran
OUT=$(gaal who ran cargo 2>/dev/null)
echo "$OUT" | jq -e '.sessions | length > 0' >/dev/null 2>&1 && pass "T3: results" || fail "T3: results" "empty"

# T4: No args
gaal who 2>/dev/null; RC=$?
[ $RC -eq 0 ] && pass "T4: exit 0" || fail "T4: exit 0" "exit $RC"
OUT=$(gaal who 2>&1)
echo "$OUT" | grep -qi 'usage\|verb\|help' && pass "T4: help text" || fail "T4: help text" "no help"

# T5: No results
OUT=$(gaal who wrote nonexistent_file_xyz_123.rs 2>/dev/null)
echo "$OUT" | jq -e '.sessions | length == 0' >/dev/null 2>&1 && pass "T5: empty sessions" || fail "T5: empty sessions" "not empty"

# T6: Human mode
OUT=$(gaal who wrote ISSUES.md -H 2>/dev/null)
echo "$OUT" | jq . >/dev/null 2>&1 && fail "T6: not JSON" "is JSON" || pass "T6: not JSON"

# T7: Limit
OUT=$(gaal who wrote ISSUES.md --limit 2 2>/dev/null)
COUNT=$(echo "$OUT" | jq '.sessions | length')
[ "$COUNT" -le 2 ] && pass "T7: <=2 ($COUNT)" || fail "T7: <=2" "$COUNT"

# T8: Token budget
BYTES=$(gaal who wrote ISSUES.md 2>/dev/null | wc -c | tr -d ' ')
[ "$BYTES" -lt 2000 ] && pass "T8: under budget ($BYTES bytes)" || fail "T8: under budget" "$BYTES bytes"

# T9: Codex subjects
OUT=$(gaal who wrote Cargo.toml --engine codex 2>/dev/null || echo '{"sessions":[]}')
PATCH=$(echo "$OUT" | jq -r '.sessions[].subjects[]?' 2>/dev/null | grep 'Begin Patch' || true)
[ -z "$PATCH" ] && pass "T9: no patch strings" || fail "T9: no patch strings" "found patches"

# T10: Query window
OUT=$(gaal who wrote ISSUES.md 2>/dev/null)
echo "$OUT" | jq -e '.query_window.from' >/dev/null 2>&1 && pass "T10: query_window.from" || fail "T10: query_window.from" "missing"
echo "$OUT" | jq -e '.query_window.to' >/dev/null 2>&1 && pass "T10: query_window.to" || fail "T10: query_window.to" "missing"

# T11: Since changes window
OUT1=$(gaal who wrote ISSUES.md --since 3d 2>/dev/null)
OUT2=$(gaal who wrote ISSUES.md --since 30d 2>/dev/null)
FROM1=$(echo "$OUT1" | jq -r '.query_window.from')
FROM2=$(echo "$OUT2" | jq -r '.query_window.from')
[ "$FROM1" != "$FROM2" ] && pass "T11: different windows ($FROM1 vs $FROM2)" || fail "T11: different windows" "same: $FROM1"

echo ""
echo "=== Suite 4: search (5 tests) ==="

# T1: Basic (search now returns envelope)
OUT=$(gaal search "handoff" 2>/dev/null)
# search may return envelope or bare array -- check both
SCORE=$(echo "$OUT" | jq -e '.results[0].score // .[0].score' 2>/dev/null)
[ -n "$SCORE" ] && [ "$SCORE" != "null" ] && pass "T1: has results with score" || fail "T1: has results with score" "no score"

# T2: Field filter
OUT=$(gaal search "cargo" --field commands 2>/dev/null)
echo "$OUT" | jq -e '(.results // .) | all(.fact_type == "command")' >/dev/null 2>&1 && pass "T2: all command type" || fail "T2: all command type" "mixed types"

# T3: Limit
OUT=$(gaal search "gaal" --limit 3 2>/dev/null)
COUNT=$(echo "$OUT" | jq '(.results // .) | length')
[ "$COUNT" -le 3 ] && pass "T3: <=3 results ($COUNT)" || fail "T3: <=3 results" "$COUNT"

# T4: No results
OUT=$(gaal search "xyzzy123nonexistent" 2>/dev/null)
echo "$OUT" | jq -e '(.results // .) | length == 0' >/dev/null 2>&1 && pass "T4: empty array" || fail "T4: empty array" "not empty"

# T5: Token budget
BYTES=$(gaal search "gaal" --limit 10 2>/dev/null | wc -c | tr -d ' ')
[ "$BYTES" -lt 5000 ] && pass "T5: under budget ($BYTES bytes)" || fail "T5: under budget" "$BYTES bytes"

echo ""
echo "=== Suite 5: recall (5 tests) ==="

# T1: Topic query
OUT=$(gaal recall "gaal issues" 2>/dev/null)
echo "$OUT" | grep -qi 'session\|headline' && pass "T1: has content" || fail "T1: has content" "empty"

# T2: No query
OUT=$(gaal recall 2>/dev/null)
echo "$OUT" | grep -qi 'session\|headline' && pass "T2: has content" || fail "T2: has content" "empty"

# T3: Brief format
BYTES=$(gaal recall "gaal" --format brief 2>/dev/null | wc -c | tr -d ' ')
[ "$BYTES" -lt 5000 ] && pass "T3: under budget ($BYTES bytes)" || fail "T3: under budget" "$BYTES bytes"

# T4: Summary format
OUT=$(gaal recall "gaal" --format summary 2>/dev/null)
echo "$OUT" | jq -e '.[0].session_id' >/dev/null 2>&1 && pass "T4: valid JSON array" || fail "T4: valid JSON array" "invalid"

# T5: Substance filter
OUT=$(gaal recall --substance 2 --format summary 2>/dev/null)
echo "$OUT" | jq -e 'all(.substance >= 2)' >/dev/null 2>&1 && pass "T5: all substance >= 2" || fail "T5: all substance >= 2" "mixed"

echo ""
echo "=== Suite 6: salt + find-salt (3 tests) ==="

# T1: Generate
SALT=$(gaal salt 2>/dev/null)
echo "$SALT" | grep -qE '^GAAL_SALT_[a-f0-9]{16}$' && pass "T1: valid format ($SALT)" || fail "T1: valid format" "$SALT"

# T2: Find (separate invocation)
sleep 1
OUT=$(gaal find-salt "$SALT" 2>/dev/null || echo '{}')
if echo "$OUT" | jq -e '.engine' >/dev/null 2>&1; then
  pass "T2: has engine"
  echo "$OUT" | jq -e '.session_id' >/dev/null 2>&1 && pass "T2: has session_id" || fail "T2: has session_id" "missing"
  echo "$OUT" | jq -e '.jsonl_path' >/dev/null 2>&1 && pass "T2: has jsonl_path" || fail "T2: has jsonl_path" "missing"
else
  skip "T2: find-salt (no active session)"
fi

# T3: Not found
gaal find-salt GAAL_SALT_0000000000000000 2>/dev/null; RC=$?
[ $RC -ne 0 ] && pass "T3: not found exits non-zero" || fail "T3: not found" "exited 0"

echo ""
echo "=== Suite 7: tag (3 tests) ==="

# T1: Add tag
gaal tag "$ID" _test_tag_v010 2>/dev/null
OUT=$(gaal inspect "$ID" 2>/dev/null)
echo "$OUT" | jq -e '.tags | index("_test_tag_v010")' >/dev/null 2>&1 && pass "T1: tag added" || fail "T1: tag added" "not found"

# T2: Remove tag
gaal tag "$ID" _test_tag_v010 --remove 2>/dev/null
OUT=$(gaal inspect "$ID" 2>/dev/null)
echo "$OUT" | jq -e '.tags | index("_test_tag_v010") | not' >/dev/null 2>&1 && pass "T2: tag removed" || fail "T2: tag removed" "still present"

# T3: List tags
OUT=$(gaal tag ls 2>/dev/null)
echo "$OUT" | jq -e 'type == "array"' >/dev/null 2>&1 && pass "T3: tag ls array" || fail "T3: tag ls array" "not array"

echo ""
echo "=== Suite 8: Cross-cutting (6 tests) ==="

# T1: No status in ls and inspect
OUT_LS=$(gaal ls 2>/dev/null)
echo "$OUT_LS" | jq -e '.sessions[0].status' >/dev/null 2>&1 && fail "T1: ls no status" "has status" || pass "T1: ls no status"
OUT_INSPECT=$(gaal inspect "$ID" 2>/dev/null)
echo "$OUT_INSPECT" | jq -e '.status' >/dev/null 2>&1 && fail "T1: inspect no status" "has status" || pass "T1: inspect no status"

# T2: show dead
gaal show 2>/dev/null; [ $? -ne 0 ] && pass "T2: show is dead" || fail "T2: show is dead" "succeeded"

# T3: active dead
gaal inspect --active 2>/dev/null; [ $? -ne 0 ] && pass "T3: --active is dead" || fail "T3: --active is dead" "succeeded"

# T4: JSON valid
gaal ls 2>/dev/null | jq . >/dev/null 2>&1 && pass "T4: ls valid JSON" || fail "T4: ls valid JSON" "invalid"
gaal search gaal 2>/dev/null | jq . >/dev/null 2>&1 && pass "T4: search valid JSON" || fail "T4: search valid JSON" "invalid"
gaal who wrote ISSUES.md 2>/dev/null | jq . >/dev/null 2>&1 && pass "T4: who valid JSON" || fail "T4: who valid JSON" "invalid"

# T5: Human not JSON
gaal ls -H 2>/dev/null | jq . >/dev/null 2>&1 && fail "T5: ls -H not JSON" "is JSON" || pass "T5: ls -H not JSON"
gaal inspect "$ID" -H 2>/dev/null | jq . >/dev/null 2>&1 && fail "T5: inspect -H not JSON" "is JSON" || pass "T5: inspect -H not JSON"
gaal who wrote ISSUES.md -H 2>/dev/null | jq . >/dev/null 2>&1 && fail "T5: who -H not JSON" "is JSON" || pass "T5: who -H not JSON"

# T6: Query window present
gaal ls 2>/dev/null | jq -e '.query_window' >/dev/null 2>&1 && pass "T6: ls query_window" || fail "T6: ls query_window" "missing"
gaal who wrote ISSUES.md 2>/dev/null | jq -e '.query_window' >/dev/null 2>&1 && pass "T6: who query_window" || fail "T6: who query_window" "missing"

echo ""
echo "==========================================="
echo "TOTAL: $PASS passed, $FAIL failed, $SKIP skipped"
echo "==========================================="
[ "$FAIL" -eq 0 ]

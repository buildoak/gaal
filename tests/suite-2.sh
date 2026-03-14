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

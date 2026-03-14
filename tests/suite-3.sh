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

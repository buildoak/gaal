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

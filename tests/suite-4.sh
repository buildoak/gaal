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

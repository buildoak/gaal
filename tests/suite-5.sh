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

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

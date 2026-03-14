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

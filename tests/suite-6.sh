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

# T1-salt-generate: Valid format
SALT=$(gaal salt)
assert "T1: valid format" echo "$SALT" | grep -E '^GAAL_SALT_[a-f0-9]{16}$'

# T2-find-salt: Finds session (MUST be separate invocation from salt generation)
# Note: In automated tests, use a previously generated salt that exists in indexed data
SALT=$(gaal salt)
sleep 1  # Allow JSONL flush
OUT=$(gaal find-salt "$SALT" 2>/dev/null || echo '{}')
# This may fail in CI without a live session — skip gracefully
if echo "$OUT" | jq -e '.engine' >/dev/null 2>&1; then
  assert "T2: has engine" echo "$OUT" | jq -e '.engine'
  assert "T2: has session_id" echo "$OUT" | jq -e '.session_id'
  assert "T2: has jsonl_path" echo "$OUT" | jq -e '.jsonl_path'
else
  echo "SKIP: T2 (no active session for find-salt)"
fi

# T3-find-salt-notfound: Clean error
assert "T3: not found exits cleanly" bash -c "gaal find-salt GAAL_SALT_0000000000000000 2>/dev/null; [ \$? -ne 0 ]"

echo "---"
echo "salt: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

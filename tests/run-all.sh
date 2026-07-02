#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TMP_ROOT=""

cleanup() {
  if [[ -n "${TMP_ROOT:-}" && -d "$TMP_ROOT" ]]; then
    rm -rf "$TMP_ROOT"
  fi
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

section() {
  printf '\n== %s ==\n' "$1"
}

run() {
  echo "+ $*"
  "$@"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

capture_gaal() {
  local name="$1"
  shift
  local out="$TMP_ROOT/$name.out"
  local err="$TMP_ROOT/$name.err"

  echo "+ gaal $*"
  if ! HOME="$TMP_ROOT/home" \
    GAAL_HOME="$TMP_ROOT/gaal-home" \
    HERMES_HOME="$TMP_ROOT/hermes-home" \
    HERMES_STATE_DB="$TMP_ROOT/hermes-home/state.db" \
    "$BIN" "$@" >"$out" 2>"$err"; then
    echo "stdout:" >&2
    sed -n '1,120p' "$out" >&2 || true
    echo "stderr:" >&2
    sed -n '1,120p' "$err" >&2 || true
    fail "gaal $* failed unexpectedly"
  fi

  CAPTURED_OUT="$out"
  CAPTURED_ERR="$err"
}

expect_gaal_failure() {
  local name="$1"
  local expected_rc="$2"
  local expected_text="$3"
  shift 3
  local out="$TMP_ROOT/$name.out"
  local err="$TMP_ROOT/$name.err"
  local combined="$TMP_ROOT/$name.combined"
  local rc

  echo "+ gaal $*  # expected failure $expected_rc"
  set +e
  HOME="$TMP_ROOT/home" \
    GAAL_HOME="$TMP_ROOT/gaal-home" \
    HERMES_HOME="$TMP_ROOT/hermes-home" \
    HERMES_STATE_DB="$TMP_ROOT/hermes-home/state.db" \
    "$BIN" "$@" >"$out" 2>"$err"
  rc=$?
  set -e

  if [[ "$rc" -ne "$expected_rc" ]]; then
    echo "stdout:" >&2
    sed -n '1,120p' "$out" >&2 || true
    echo "stderr:" >&2
    sed -n '1,120p' "$err" >&2 || true
    fail "gaal $* exited $rc, expected $expected_rc"
  fi

  cat "$out" "$err" >"$combined"

  if ! grep -Fq -- "$expected_text" "$combined"; then
    echo "stdout:" >&2
    sed -n '1,120p' "$out" >&2 || true
    echo "stderr:" >&2
    sed -n '1,120p' "$err" >&2 || true
    fail "gaal $* did not mention: $expected_text"
  fi

  CAPTURED_OUT="$combined"
  CAPTURED_ERR="$err"
}

assert_contains() {
  local file="$1"
  local text="$2"
  grep -Fq -- "$text" "$file" || fail "$file does not contain: $text"
}

assert_json_expr() {
  local label="$1"
  local file="$2"
  local expr="$3"

  python3 - "$label" "$file" "$expr" <<'PY'
import json
import sys

label, path, expr = sys.argv[1], sys.argv[2], sys.argv[3]
with open(path, "r", encoding="utf-8") as handle:
    data = json.load(handle)

if not eval(expr, {"__builtins__": {}}, {"data": data}):
    raise SystemExit(f"{label}: assertion failed: {expr}\n{data!r}")
PY
}

require_command cargo
require_command python3

section "Rust gate"
run cargo fmt --check
run cargo check --all-targets
run cargo test
run cargo build

section "Install and scheduler gate"
run tests/install-scheduler.sh

BIN="$ROOT/target/debug/gaal"
[[ -x "$BIN" ]] || fail "debug binary was not built at $BIN"

section "Isolated first-run smoke"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/gaal-run-all.XXXXXX")"
mkdir -p "$TMP_ROOT/home" "$TMP_ROOT/gaal-home" "$TMP_ROOT/hermes-home"

capture_gaal version --version
assert_contains "$CAPTURED_OUT" "gaal "

capture_gaal top-help --help
assert_contains "$CAPTURED_OUT" "Agent session observability CLI"
assert_contains "$CAPTURED_OUT" "New here?"
assert_contains "$CAPTURED_OUT" "gaal onboard --dry-run"
assert_contains "$CAPTURED_OUT" "gaal onboard"
assert_contains "$CAPTURED_OUT" "index"
assert_contains "$CAPTURED_OUT" "ls"

capture_gaal onboard-dry-run onboard --dry-run
assert_json_expr "onboard dry run" "$CAPTURED_OUT" \
  'data["kind"] == "onboarding" and data["dry_run"] == True and data["skill"]["directory_url"].endswith("/gaal/tree/master/skill") and "gaal index backfill" in data["first_launch"]["commands"]'

capture_gaal onboard-human -H onboard --dry-run
assert_contains "$CAPTURED_OUT" "Agent install instruction:"
assert_contains "$CAPTURED_OUT" "https://github.com/buildoak/gaal/tree/master/skill"
assert_contains "$CAPTURED_OUT" "gaal index backfill"

capture_gaal index-help index --help
assert_contains "$CAPTURED_OUT" "backfill"
assert_contains "$CAPTURED_OUT" "status"

capture_gaal ls-help ls --help
assert_contains "$CAPTURED_OUT" "Fleet view across sessions"
assert_contains "$CAPTURED_OUT" "--aggregate"

capture_gaal status-before index status
assert_json_expr "empty status before backfill" "$CAPTURED_OUT" \
  'data["sessions_total"] == 0 and data["facts_total"] == 0 and data["handoffs_total"] == 0 and data["db_path"].endswith("/gaal-home/index.db")'

capture_gaal backfill index backfill
assert_json_expr "empty backfill summary" "$CAPTURED_OUT" \
  'data["indexed"] == 0 and data["skipped"] == 0 and data["errors"] == 0'

capture_gaal status-after index status
assert_json_expr "empty status after backfill" "$CAPTURED_OUT" \
  'data["sessions_total"] == 0 and data["facts_total"] == 0 and data["handoffs_total"] == 0 and data["db_path"].endswith("/gaal-home/index.db")'

capture_gaal aggregate ls --aggregate
assert_json_expr "empty aggregate" "$CAPTURED_OUT" \
  'data["sessions"] == 0 and data["total_input_tokens"] == 0 and data["total_output_tokens"] == 0 and data["by_engine"] == {}'

capture_gaal tag-list tag ls
assert_json_expr "empty tag list" "$CAPTURED_OUT" 'data == []'

expect_gaal_failure ls-empty-json 1 "No sessions are indexed yet." ls --limit 5
assert_json_expr "empty ls JSON error" "$CAPTURED_OUT" \
  'data["ok"] == False and data["exit_code"] == 1 and "gaal index backfill" in data["example"]'

expect_gaal_failure ls-empty-human 1 "No sessions are indexed yet." ls -H --limit 5
assert_contains "$CAPTURED_OUT" "What went wrong:"
assert_contains "$CAPTURED_OUT" "Hint:"

section "Legacy shell suites intentionally excluded"
cat <<'EOF'
tests/suite-1.sh through tests/suite-8.sh are not part of this launch gate.
They depend on private/current indexed sessions, generated handoffs, mutable tags
on real sessions, and live salt discovery. The deterministic coverage for parser,
database, fixture, and command behavior lives in cargo test plus the isolated
first-run CLI smoke above.
EOF

section "PASS"
echo "./tests/run-all.sh completed without touching real ~/.gaal or LaunchAgents"

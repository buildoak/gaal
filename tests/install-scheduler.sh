#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_PLIST="${TMPDIR:-/tmp}/gaal-install-scheduler-test.$$.$RANDOM.plist"
trap 'rm -f "${TMP_PLIST}"' EXIT

cd "${ROOT_DIR}"

./install.sh --help >/dev/null
./install.sh --dry-run >/dev/null
./install.sh handoff-setup --dry-run --no-agent-mux >/dev/null

defaults/gaal-cron-install.sh --help >/dev/null
defaults/gaal-cron-install.sh --print-plist > "${TMP_PLIST}"

if command -v plutil >/dev/null 2>&1; then
  plutil -lint "${TMP_PLIST}" >/dev/null
fi

grep -q '<string>index</string>' "${TMP_PLIST}"
grep -q '<string>backfill</string>' "${TMP_PLIST}"
if [ "$(grep -c '<string>backfill</string>' "${TMP_PLIST}")" -ne 1 ]; then
  echo "generated scheduler plist must contain exactly one backfill command" >&2
  exit 1
fi
if grep -E '<string>--engine</string>|<string>grok</string>' "${TMP_PLIST}" >/dev/null; then
  echo "generated scheduler plist must rely on unfiltered default discovery, including Grok" >&2
  exit 1
fi
if grep -E 'create-handoff|recall|agent-mux|2&gt;/dev/null|2>/dev/null' "${TMP_PLIST}" >/dev/null; then
  echo "generated scheduler plist contains forbidden scheduled work" >&2
  exit 1
fi
if grep -q '/ABSOLUTE/PATH/TO/gaal' "${TMP_PLIST}"; then
  echo "generated scheduler plist kept the template placeholder" >&2
  exit 1
fi

echo "install/scheduler checks passed"

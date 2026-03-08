#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLIST_NAME="com.gaal.index.plist"
SOURCE_PLIST="${SCRIPT_DIR}/${PLIST_NAME}"
DEST_DIR="${HOME}/Library/LaunchAgents"
DEST_PLIST="${DEST_DIR}/${PLIST_NAME}"

mkdir -p "${DEST_DIR}"
cp "${SOURCE_PLIST}" "${DEST_PLIST}"

launchctl unload "${DEST_PLIST}" 2>/dev/null || true
launchctl load "${DEST_PLIST}"

#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

LABEL="com.gaal.index"
PLIST_NAME="${LABEL}.plist"
DEST_DIR="${HOME}/Library/LaunchAgents"
DEST_PLIST="${DEST_DIR}/${PLIST_NAME}"
LOG_DIR="${HOME}/Library/Logs/gaal"
INTERVAL="14400"
GAAL_BIN=""
GAAL_HOME_VALUE=""
DRY_RUN=0
COMMAND="help"

usage() {
  cat <<'EOF'
Usage: defaults/gaal-cron-install.sh <command> [options]
       defaults/gaal-cron-install.sh --print-plist [options]

Commands:
  install              Write and load the LaunchAgent
  uninstall            Unload and remove the LaunchAgent
  status               Show installed plist and launchctl status
  print-plist          Print the generated plist to stdout without installing
  help                 Show this help

Options:
  --gaal PATH          Gaal binary to schedule; defaults to resolved gaal path
  --interval SECONDS   LaunchAgent StartInterval; default 14400
  --log-dir PATH       Log directory; default ~/Library/Logs/gaal
  --gaal-home PATH     Set GAAL_HOME for scheduled runs
  --dry-run            Show intended install/uninstall actions without mutating
  -h, --help           Show this help

The scheduled job runs only:
  gaal index backfill

That single unfiltered command includes Grok and every other supported engine;
the installer does not add a second Grok-only job.

It does not create handoffs, call LLM backends, run recall, or perform broad
maintenance.
EOF
}

log() {
  printf '%s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

xml_escape() {
  printf '%s' "$1" | sed \
    -e 's/&/\&amp;/g' \
    -e 's/</\&lt;/g' \
    -e 's/>/\&gt;/g' \
    -e 's/"/\&quot;/g' \
    -e "s/'/\&apos;/g"
}

absolute_path() {
  path="$1"
  case "${path}" in
    /*)
      printf '%s\n' "${path}"
      ;;
    *)
      dir="$(dirname "${path}")"
      base="$(basename "${path}")"
      printf '%s/%s\n' "$(cd "${dir}" && pwd -P)" "${base}"
      ;;
  esac
}

resolve_gaal() {
  if [ -n "${GAAL_BIN}" ]; then
    absolute_path "${GAAL_BIN}"
    return 0
  fi

  if command -v gaal >/dev/null 2>&1; then
    command -v gaal
    return 0
  fi

  if [ -x "${HOME}/.cargo/bin/gaal" ]; then
    printf '%s\n' "${HOME}/.cargo/bin/gaal"
    return 0
  fi

  if [ -x "${REPO_DIR}/target/release/gaal" ]; then
    printf '%s\n' "${REPO_DIR}/target/release/gaal"
    return 0
  fi

  return 1
}

validate_binary() {
  bin="$1"
  [ -x "${bin}" ] || die "gaal binary is not executable: ${bin}"
  "${bin}" --version >/dev/null
}

preview_gaal() {
  if bin="$(resolve_gaal)"; then
    printf '%s\n' "${bin}"
  else
    warn "could not resolve gaal; rendering expected Cargo install path"
    printf '%s\n' "${HOME}/.cargo/bin/gaal"
  fi
}

generate_plist() {
  bin="$1"
  esc_bin="$(xml_escape "${bin}")"
  esc_out="$(xml_escape "${LOG_DIR}/index.out.log")"
  esc_err="$(xml_escape "${LOG_DIR}/index.err.log")"
  esc_label="$(xml_escape "${LABEL}")"
  esc_home="$(xml_escape "${GAAL_HOME_VALUE}")"

  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${esc_label}</string>
EOF

  if [ -n "${GAAL_HOME_VALUE}" ]; then
    cat <<EOF
    <key>EnvironmentVariables</key>
    <dict>
        <key>GAAL_HOME</key>
        <string>${esc_home}</string>
    </dict>
EOF
  fi

  cat <<EOF
    <key>ProgramArguments</key>
    <array>
        <string>${esc_bin}</string>
        <string>index</string>
        <string>backfill</string>
    </array>
    <key>StartInterval</key>
    <integer>${INTERVAL}</integer>
    <key>StandardOutPath</key>
    <string>${esc_out}</string>
    <key>StandardErrorPath</key>
    <string>${esc_err}</string>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
EOF
}

lint_plist() {
  plist="$1"
  if command -v plutil >/dev/null 2>&1; then
    plutil -lint "${plist}"
  fi
}

make_temp_plist() {
  bin="$1"
  tmp="${TMPDIR:-/tmp}/gaal-index.$$.$RANDOM.plist"
  generate_plist "${bin}" > "${tmp}"
  lint_plist "${tmp}" >/dev/null
  printf '%s\n' "${tmp}"
}

print_plist() {
  bin="$(preview_gaal)"
  if [ -x "${bin}" ]; then
    validate_binary "${bin}"
  else
    warn "gaal binary is not executable yet: ${bin}"
  fi
  generate_plist "${bin}"
}

install_agent() {
  if [ "${DRY_RUN}" -eq 1 ]; then
    bin="$(preview_gaal)"
    if [ -x "${bin}" ]; then
      validate_binary "${bin}"
    else
      warn "gaal binary is not executable yet: ${bin}"
    fi
    log "Would install LaunchAgent ${LABEL}"
    log "Destination: ${DEST_PLIST}"
    log "Binary: ${bin}"
    log "Logs: ${LOG_DIR}/index.out.log and ${LOG_DIR}/index.err.log"
    generate_plist "${bin}"
    return 0
  fi

  bin="$(resolve_gaal)" || die "could not resolve gaal; pass --gaal PATH"
  validate_binary "${bin}"

  tmp="$(make_temp_plist "${bin}")"
  trap 'rm -f "${tmp}"' EXIT

  mkdir -p "${DEST_DIR}" "${LOG_DIR}"
  cp "${tmp}" "${DEST_PLIST}"
  lint_plist "${DEST_PLIST}"

  if launchctl list "${LABEL}" >/dev/null 2>&1; then
    launchctl unload "${DEST_PLIST}" || warn "existing LaunchAgent unload failed; continuing with load"
  fi
  launchctl load "${DEST_PLIST}"

  log "Installed ${LABEL}"
  log "Plist: ${DEST_PLIST}"
  log "Binary: ${bin}"
  log "Logs: ${LOG_DIR}/index.out.log and ${LOG_DIR}/index.err.log"
}

uninstall_agent() {
  if [ "${DRY_RUN}" -eq 1 ]; then
    log "Would unload LaunchAgent ${LABEL} if loaded"
    log "Would remove ${DEST_PLIST}"
    return 0
  fi

  if [ -f "${DEST_PLIST}" ]; then
    if launchctl list "${LABEL}" >/dev/null 2>&1; then
      launchctl unload "${DEST_PLIST}" || warn "LaunchAgent unload failed"
    fi
    rm -f "${DEST_PLIST}"
    log "Removed ${DEST_PLIST}"
  else
    log "No installed plist at ${DEST_PLIST}"
  fi
}

status_agent() {
  log "Label: ${LABEL}"
  log "Destination: ${DEST_PLIST}"

  if [ -f "${DEST_PLIST}" ]; then
    log "Plist: installed"
    lint_plist "${DEST_PLIST}" || true
  else
    log "Plist: not installed"
  fi

  if launchctl list "${LABEL}" >/dev/null 2>&1; then
    log "LaunchAgent: loaded"
    launchctl list "${LABEL}" || true
  else
    log "LaunchAgent: not loaded"
  fi

  if bin="$(resolve_gaal)"; then
    log "Resolved gaal: ${bin}"
    "${bin}" --version || warn "gaal --version failed"
  else
    warn "could not resolve gaal binary"
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    install|uninstall|status|print-plist|help)
      COMMAND="$1"
      ;;
    --print-plist)
      COMMAND="print-plist"
      ;;
    --gaal)
      shift
      [ "$#" -gt 0 ] || die "--gaal requires a path"
      GAAL_BIN="$1"
      ;;
    --interval)
      shift
      [ "$#" -gt 0 ] || die "--interval requires seconds"
      INTERVAL="$1"
      ;;
    --log-dir)
      shift
      [ "$#" -gt 0 ] || die "--log-dir requires a path"
      LOG_DIR="$1"
      ;;
    --gaal-home)
      shift
      [ "$#" -gt 0 ] || die "--gaal-home requires a path"
      GAAL_HOME_VALUE="$1"
      ;;
    --dry-run)
      DRY_RUN=1
      ;;
    -h|--help)
      COMMAND="help"
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
  shift
done

case "${COMMAND}" in
  install)
    install_agent
    ;;
  uninstall)
    uninstall_agent
    ;;
  status)
    status_agent
    ;;
  print-plist)
    print_plist
    ;;
  help)
    usage
    ;;
  *)
    die "unknown command: ${COMMAND}"
    ;;
esac

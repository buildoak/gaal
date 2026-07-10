#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCHEDULER="${ROOT_DIR}/defaults/gaal-cron-install.sh"

DRY_RUN=0
SCHEDULE_MODE="auto"
SKIP_INDEX=0
ACTION="install"
HANDOFF_SETUP=0
AGENT_MUX_INSTALL_MODE="auto"
AGENT_MUX_REPO_URL="${AGENT_MUX_REPO_URL:-https://github.com/buildoak/agent-mux.git}"
AGENT_MUX_SOURCE_DIR="${AGENT_MUX_SOURCE_DIR:-${HOME}/.local/src/agent-mux}"
AGENT_MUX_BIN="${AGENT_MUX_BIN:-${HOME}/.local/bin/agent-mux}"
AGENT_MUX_RESOLVED=""
GAAL_BIN_FOR_HANDOFF=""

usage() {
  cat <<'EOF'
Usage: ./install.sh [install|status|next-steps|handoff-setup|print-plist|uninstall-schedule] [options]

Install Gaal from this source checkout and run the first local index.

Actions:
  install              Build/install from source, verify, index, and optionally schedule
  status               Show local toolchain, binary, version, and schedule status
  next-steps           Print first useful commands and prompts
  handoff-setup        Explain and validate optional agent-mux handoff backend setup
  print-plist          Print the scheduled indexing LaunchAgent plist
  uninstall-schedule   Unload/remove the scheduled indexing LaunchAgent
  help                 Show this help

Options:
  --schedule           Install scheduled indexing after source install
  --no-schedule        Do not prompt or install scheduled indexing
  --skip-index         Install/verify the binary but skip first index backfill
  --setup-handoffs     After Gaal setup, run the optional handoff backend setup
  --install-agent-mux  Explicitly approve source install of agent-mux if missing
  --no-agent-mux       Do not prompt or install agent-mux; only explain handoff setup
  --print-next-steps   Alias for next-steps
  --dry-run            Print planned commands without changing files or LaunchAgents
  -h, --help           Show this help

Scheduling is explicit. In a non-interactive shell, this script never installs
the LaunchAgent unless --schedule is provided.

Handoff setup is also explicit. It can validate agent-mux and run dry-run
handoff planning, but it never generates handoffs unless you later run a
non-dry-run gaal create-handoff command yourself.
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

run_cmd() {
  if [ "${DRY_RUN}" -eq 1 ]; then
    local line="+"
    while [ "$#" -gt 0 ]; do
      line="${line} $1"
      shift
    done
    printf '%s\n' "${line}"
  else
    "$@"
  fi
}

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

cargo_install_bin() {
  local cargo_root
  cargo_root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-${HOME}/.cargo}}"
  printf '%s/bin/gaal\n' "${cargo_root}"
}

agent_mux_bin() {
  if have_cmd agent-mux; then
    command -v agent-mux
    return 0
  fi

  if [ -x "${AGENT_MUX_BIN}" ]; then
    printf '%s\n' "${AGENT_MUX_BIN}"
    return 0
  fi

  return 1
}

resolve_gaal() {
  if have_cmd gaal; then
    command -v gaal
    return 0
  fi

  if [ -x "${HOME}/.cargo/bin/gaal" ]; then
    printf '%s\n' "${HOME}/.cargo/bin/gaal"
    return 0
  fi

  if [ -x "${ROOT_DIR}/target/release/gaal" ]; then
    printf '%s\n' "${ROOT_DIR}/target/release/gaal"
    return 0
  fi

  return 1
}

print_next_steps() {
  cat <<'EOF'
Next steps:
  gaal index backfill
  gaal index status
  gaal ls -H --limit 5
  gaal inspect latest --tokens -H

If no sessions are listed, run Codex CLI, Claude Code, Gemini CLI, Antigravity
CLI, Hermes, or Grok Build locally once, then run:
  gaal index backfill

Recommended scheduled indexing:
  ./install.sh --schedule
  defaults/gaal-cron-install.sh status
  defaults/gaal-cron-install.sh --print-plist

Safe handoff preview:
  gaal create-handoff latest --dry-run

Handoffs are optional. Do not generate them until the dry-run provider, side
effects, and planned path look right.

Optional handoff backend setup:
  ./install.sh handoff-setup
  ./install.sh handoff-setup --install-agent-mux
EOF
}

gaal_state_dir() {
  printf '%s\n' "${GAAL_HOME:-${HOME}/.gaal}"
}

print_install_plan() {
  cat <<EOF
What this installer is about to do:
  1. Build and install Gaal from this source checkout with Cargo.
  2. Verify the exact installed Gaal binary.
  3. Create or refresh Gaal's local derived index under $(gaal_state_dir).
  4. Show the first sessions if any exist; explain the clean zero-session case if none exist.
  5. Offer scheduled indexing only when explicitly approved.

What it will not do by default:
  - It will not install a LaunchAgent unless you pass --schedule or accept a TTY prompt.
  - It will not install agent-mux unless you approve handoff setup.
  - It will not generate handoffs or call an LLM backend.
  - It will not move source traces from Codex, Claude Code, Gemini, agy, Hermes, or Grok.
EOF

  if [ "${SCHEDULE_MODE}" = "yes" ]; then
    log "You passed --schedule, so scheduled indexing is approved for this run."
  elif [ "${SCHEDULE_MODE}" = "no" ]; then
    log "You passed --no-schedule, so scheduled indexing will be skipped."
  else
    log "Scheduling mode: ask in an interactive terminal; skip in non-interactive shells."
  fi

  if [ "${HANDOFF_SETUP}" -eq 1 ]; then
    log "You asked for optional handoff setup. It will validate agent-mux and run dry-run planning only."
  else
    log "Optional handoff setup is available later with: ./install.sh handoff-setup"
  fi
}

print_handoff_setup_intro() {
  cat <<'EOF'
Optional handoff setup:
  Gaal can generate compact continuity handoffs, but that is separate from
  indexing. Handoffs use an LLM/agent backend and may spend quota or local
  compute, so this setup is explicit.

What this step may do:
  1. Check whether agent-mux is installed.
  2. If agent-mux is missing, install it from source only after direct approval.
  3. Show agent-mux profiles and engine configuration when available.
  4. Run `gaal create-handoff latest --dry-run` to preview provider/model/side effects.

What this step will not do:
  - It will not generate a real handoff.
  - It will not run `gaal create-handoff` without `--dry-run`.
  - It will not schedule handoff generation.
EOF
}

approve_agent_mux_install() {
  case "${AGENT_MUX_INSTALL_MODE}" in
    yes)
      return 0
      ;;
    no)
      return 1
      ;;
    auto)
      if [ -t 0 ]; then
        printf 'Install agent-mux from source now? This runs git clone/go build into ~/.local. [y/N] '
        read -r answer
        case "${answer}" in
          y|Y|yes|YES)
            return 0
            ;;
        esac
      fi
      return 1
      ;;
    *)
      die "internal error: unknown agent-mux install mode ${AGENT_MUX_INSTALL_MODE}"
      ;;
  esac
}

install_agent_mux_if_approved() {
  if mux_bin="$(agent_mux_bin)"; then
    AGENT_MUX_RESOLVED="${mux_bin}"
    return 0
  fi

  warn "agent-mux is not installed or not on PATH."
  if ! approve_agent_mux_install; then
    cat <<EOF
agent-mux setup skipped.

To approve it explicitly later:
  ./install.sh handoff-setup --install-agent-mux

Manual source install:
  git clone ${AGENT_MUX_REPO_URL} "${AGENT_MUX_SOURCE_DIR}"
  cd "${AGENT_MUX_SOURCE_DIR}"
  go build -o "${AGENT_MUX_BIN}" ./cmd/agent-mux
EOF
    return 1
  fi

  have_cmd git || die "git is required to install agent-mux from source"
  have_cmd go || die "go is required to build agent-mux from source"

  if [ "${DRY_RUN}" -eq 1 ]; then
    run_cmd mkdir -p "$(dirname "${AGENT_MUX_SOURCE_DIR}")" "$(dirname "${AGENT_MUX_BIN}")"
    if [ ! -d "${AGENT_MUX_SOURCE_DIR}/.git" ]; then
      run_cmd git clone "${AGENT_MUX_REPO_URL}" "${AGENT_MUX_SOURCE_DIR}"
    fi
    run_cmd cd "${AGENT_MUX_SOURCE_DIR}"
    run_cmd go build -o "${AGENT_MUX_BIN}" ./cmd/agent-mux
    AGENT_MUX_RESOLVED="${AGENT_MUX_BIN}"
    return 0
  fi

  mkdir -p "$(dirname "${AGENT_MUX_SOURCE_DIR}")" "$(dirname "${AGENT_MUX_BIN}")"
  if [ ! -d "${AGENT_MUX_SOURCE_DIR}/.git" ]; then
    git clone "${AGENT_MUX_REPO_URL}" "${AGENT_MUX_SOURCE_DIR}"
  else
    log "Using existing agent-mux source checkout: ${AGENT_MUX_SOURCE_DIR}"
  fi

  (cd "${AGENT_MUX_SOURCE_DIR}" && go build -o "${AGENT_MUX_BIN}" ./cmd/agent-mux)
  AGENT_MUX_RESOLVED="${AGENT_MUX_BIN}"
}

run_handoff_setup() {
  print_handoff_setup_intro

  if ! install_agent_mux_if_approved; then
    log "Core Gaal is ready without handoffs. Handoff generation remains preview-only until agent-mux is installed."
    return 0
  fi
  mux_bin="${AGENT_MUX_RESOLVED}"

  log "agent-mux binary: ${mux_bin}"

  if [ "${DRY_RUN}" -eq 1 ]; then
    run_cmd "${mux_bin}" config prompts
    run_cmd "${mux_bin}" config engines --json
  else
    "${mux_bin}" config prompts || warn "agent-mux profile check failed"
    "${mux_bin}" config engines --json >/dev/null || warn "agent-mux engine check failed"
  fi

  if [ -n "${GAAL_BIN_FOR_HANDOFF}" ]; then
    GAAL_BIN="${GAAL_BIN_FOR_HANDOFF}"
  elif GAAL_BIN="$(resolve_gaal)"; then
    :
  else
    GAAL_BIN=""
  fi

  if [ -n "${GAAL_BIN}" ]; then
    log "Previewing handoff generation with dry-run only."
    if [ "${DRY_RUN}" -eq 1 ]; then
      run_cmd "${GAAL_BIN}" create-handoff latest --dry-run
    else
      set +e
      "${GAAL_BIN}" create-handoff latest --dry-run
      dry_rc=$?
      set -e
      case "${dry_rc}" in
        0)
          log "Handoff dry-run succeeded. Real generation still requires a separate non-dry-run command."
          ;;
        1|3|10)
          warn "Handoff dry-run could not find a usable indexed session yet. Index sessions first, then retry."
          ;;
        *)
          warn "Handoff dry-run exited ${dry_rc}; inspect the error before generating handoffs."
          ;;
      esac
    fi
  else
    warn "gaal binary not found; install Gaal before handoff dry-run preview"
  fi
}

status() {
  log "Gaal source checkout: ${ROOT_DIR}"

  if have_cmd cargo; then
    log "cargo: $(command -v cargo)"
  else
    warn "cargo not found on PATH"
  fi

  if have_cmd rustc; then
    log "rustc: $(command -v rustc)"
  else
    warn "rustc not found on PATH"
  fi

  if GAAL_BIN="$(resolve_gaal)"; then
    log "gaal: ${GAAL_BIN}"
    if [ -x "${GAAL_BIN}" ]; then
      "${GAAL_BIN}" --version || warn "gaal --version failed"
    fi
    if ! have_cmd gaal; then
      warn "gaal is not on PATH; add the binary directory before relying on shell lookup"
    fi
  else
    warn "gaal binary not found"
  fi

  if [ -x "${SCHEDULER}" ]; then
    "${SCHEDULER}" status || true
  else
    warn "scheduler helper is not executable: ${SCHEDULER}"
  fi
}

install_schedule_if_requested() {
  GAAL_BIN="$1"
  schedule_dry_run_flag=""
  if [ "${DRY_RUN}" -eq 1 ]; then
    schedule_dry_run_flag="--dry-run"
  fi

  case "${SCHEDULE_MODE}" in
    yes)
      if [ -n "${schedule_dry_run_flag}" ]; then
        run_cmd "${SCHEDULER}" install "${schedule_dry_run_flag}" --gaal "${GAAL_BIN}"
      else
        run_cmd "${SCHEDULER}" install --gaal "${GAAL_BIN}"
      fi
      ;;
    no)
      log "Scheduled indexing skipped. Enable it later with: ./install.sh --schedule"
      ;;
    auto)
      if [ -t 0 ]; then
        printf 'Install scheduled indexing now? One unfiltered "%s index backfill" run includes Grok and all supported engines every 4 hours. [y/N] ' "${GAAL_BIN}"
        read -r answer
        case "${answer}" in
          y|Y|yes|YES)
            if [ -n "${schedule_dry_run_flag}" ]; then
              run_cmd "${SCHEDULER}" install "${schedule_dry_run_flag}" --gaal "${GAAL_BIN}"
            else
              run_cmd "${SCHEDULER}" install --gaal "${GAAL_BIN}"
            fi
            ;;
          *)
            log "Scheduled indexing skipped. Enable it later with: ./install.sh --schedule"
            ;;
        esac
      else
        log "Non-interactive shell detected; scheduled indexing not installed without --schedule."
        log "Enable it later with: ./install.sh --schedule"
      fi
      ;;
    *)
      die "internal error: unknown schedule mode ${SCHEDULE_MODE}"
      ;;
  esac
}

install() {
  log "Gaal is source-installed from this checkout. No crates.io install path is used."
  print_install_plan

  have_cmd cargo || die "cargo is required. Install Rust first: https://rustup.rs/"
  have_cmd rustc || die "rustc is required. Install Rust first: https://rustup.rs/"

  run_cmd cargo install --path "${ROOT_DIR}" --force

  if [ "${DRY_RUN}" -eq 1 ]; then
    GAAL_BIN="$(cargo_install_bin)"
  else
    GAAL_BIN="$(cargo_install_bin)"
    if [ ! -x "${GAAL_BIN}" ]; then
      die "cargo install completed but expected binary was not found at ${GAAL_BIN}"
    fi
  fi

  log "Resolved gaal binary: ${GAAL_BIN}"

  if [ "${DRY_RUN}" -eq 0 ]; then
    "${GAAL_BIN}" --version
    if ! have_cmd gaal; then
      warn "gaal is installed at ${GAAL_BIN} but is not currently on PATH"
      warn "Add $(dirname "${GAAL_BIN}") to PATH before relying on scheduled or manual shell lookup"
    fi
  else
    run_cmd "${GAAL_BIN}" --version
  fi

  log "Derived Gaal state lives under $(gaal_state_dir); source traces stay where agent tools wrote them."

  if [ "${SKIP_INDEX}" -eq 0 ]; then
    run_cmd "${GAAL_BIN}" index backfill
    run_cmd "${GAAL_BIN}" index status
    if [ "${DRY_RUN}" -eq 1 ]; then
      run_cmd "${GAAL_BIN}" ls -H --limit 5
    else
      set +e
      "${GAAL_BIN}" ls -H --limit 5
      ls_rc=$?
      set -e
      case "${ls_rc}" in
        0)
          ;;
        1)
          log "No sessions are indexed yet. That is a valid first run on a clean machine."
          log "Run Codex CLI, Claude Code, Gemini CLI, Antigravity CLI, Hermes, or Grok Build once, then run: gaal index backfill"
          ;;
        *)
          die "gaal ls failed with exit ${ls_rc}"
          ;;
      esac
    fi
  else
    log "First index skipped by --skip-index."
  fi

  install_schedule_if_requested "${GAAL_BIN}"
  if [ "${HANDOFF_SETUP}" -eq 1 ]; then
    GAAL_BIN_FOR_HANDOFF="${GAAL_BIN}"
    run_handoff_setup
  fi
  print_next_steps
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    install|status|next-steps|handoff-setup|print-plist|uninstall-schedule|help)
      ACTION="$1"
      ;;
    --schedule)
      SCHEDULE_MODE="yes"
      ;;
    --no-schedule)
      SCHEDULE_MODE="no"
      ;;
    --skip-index)
      SKIP_INDEX=1
      ;;
    --setup-handoffs)
      HANDOFF_SETUP=1
      ;;
    --install-agent-mux)
      HANDOFF_SETUP=1
      AGENT_MUX_INSTALL_MODE="yes"
      ;;
    --no-agent-mux)
      AGENT_MUX_INSTALL_MODE="no"
      ;;
    --print-next-steps)
      ACTION="next-steps"
      ;;
    --dry-run)
      DRY_RUN=1
      ;;
    -h|--help)
      ACTION="help"
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
  shift
done

case "${ACTION}" in
  install)
    install
    ;;
  status)
    status
    ;;
  next-steps)
    print_next_steps
    ;;
  handoff-setup)
    run_handoff_setup
    ;;
  print-plist)
    if [ "${DRY_RUN}" -eq 1 ]; then
      run_cmd "${SCHEDULER}" --print-plist
    else
      "${SCHEDULER}" --print-plist
    fi
    ;;
  uninstall-schedule)
    if [ "${DRY_RUN}" -eq 1 ]; then
      run_cmd "${SCHEDULER}" uninstall --dry-run
    else
      "${SCHEDULER}" uninstall
    fi
    ;;
  help)
    usage
    ;;
  *)
    die "unknown action: ${ACTION}"
    ;;
esac

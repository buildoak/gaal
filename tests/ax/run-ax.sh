#!/usr/bin/env bash
# run-ax.sh — Top-level AX test harness orchestrator
#
# Runs all three layers sequentially:
#   Layer 1: Error quality audit (generate + judge)
#   Layer 2: First-attempt agent tasks
#   Layer 3: Trace analysis
#
# Usage:
#   ./run-ax.sh                     # run everything
#   ./run-ax.sh --layer 1           # run only layer 1
#   ./run-ax.sh --layer 1 --layer 2 # run layers 1 and 2
#   ./run-ax.sh --dry-run           # preview all dispatches
#   ./run-ax.sh --skip-judge        # run layer 1 generate only (no API cost)
#   ./run-ax.sh --skip-dispatch     # generate errors, skip agent dispatches

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DRY_RUN=false
SKIP_JUDGE=false
SKIP_DISPATCH=false
LAYERS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --layer) LAYERS+=("$2"); shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        --skip-judge) SKIP_JUDGE=true; shift ;;
        --skip-dispatch) SKIP_DISPATCH=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--layer N]... [--dry-run] [--skip-judge] [--skip-dispatch]"
            echo ""
            echo "Layers:"
            echo "  1  Error quality audit (generate-errors + judge-errors)"
            echo "  2  First-attempt agent tasks"
            echo "  3  Trace analysis"
            echo ""
            echo "Options:"
            echo "  --dry-run        Preview dispatches without API calls"
            echo "  --skip-judge     Run error generation but skip the LLM judge"
            echo "  --skip-dispatch  Skip all agent-mux dispatches (Layer 1 generate only)"
            exit 0
            ;;
        *) echo "Unknown arg: $1. Use --help for usage." >&2; exit 1 ;;
    esac
done

# Default: run all layers
if [[ ${#LAYERS[@]} -eq 0 ]]; then
    LAYERS=(1 2 3)
fi

should_run() {
    local target="$1"
    for l in "${LAYERS[@]}"; do
        [[ "$l" == "$target" ]] && return 0
    done
    return 1
}

dry_run_flag=""
[[ "$DRY_RUN" == true ]] && dry_run_flag="--dry-run"

echo "╔══════════════════════════════════════════════╗"
echo "║         gaal AX Test Harness                 ║"
echo "║  Agent Experience Quality Assessment         ║"
echo "╚══════════════════════════════════════════════╝"
echo ""
echo "Layers to run: ${LAYERS[*]}"
echo "Dry run: $DRY_RUN"
echo "Timestamp: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# ─── Layer 1: Error Quality ──────────────────────────────────────────────────
if should_run 1; then
    echo "━━━ Layer 1: Error Quality Audit ━━━"
    echo ""

    echo "Step 1/2: Generating error outputs..."
    "${SCRIPT_DIR}/layer1-errors/generate-errors.sh" > "${SCRIPT_DIR}/layer1-errors/errors.json"
    echo "  Written: layer1-errors/errors.json"
    echo ""

    if [[ "$SKIP_JUDGE" == false && "$SKIP_DISPATCH" == false ]]; then
        echo "Step 2/2: Dispatching error quality judge..."
        "${SCRIPT_DIR}/layer1-errors/judge-errors.sh" $dry_run_flag
    else
        echo "Step 2/2: Skipped (--skip-judge or --skip-dispatch)"
    fi
    echo ""
fi

# ─── Layer 2: Agent Tasks ────────────────────────────────────────────────────
if should_run 2; then
    echo "━━━ Layer 2: First-Attempt Agent Tasks ━━━"
    echo ""

    if [[ "$SKIP_DISPATCH" == true ]]; then
        echo "Skipped (--skip-dispatch)"
    else
        "${SCRIPT_DIR}/layer2-tasks/run-tasks.sh" $dry_run_flag
    fi
    echo ""
fi

# ─── Layer 3: Trace Analysis ─────────────────────────────────────────────────
if should_run 3; then
    echo "━━━ Layer 3: Trace Analysis ━━━"
    echo ""

    if [[ "$SKIP_DISPATCH" == true ]]; then
        echo "Skipped (--skip-dispatch)"
    elif [[ ! -f "${SCRIPT_DIR}/layer2-tasks/results.json" ]]; then
        echo "Skipped: Layer 2 results not found. Run Layer 2 first."
    else
        "${SCRIPT_DIR}/layer3-analysis/analyze-traces.sh" $dry_run_flag
    fi
    echo ""
fi

# ─── Summary ─────────────────────────────────────────────────────────────────
echo "━━━ AX Harness Complete ━━━"
echo ""
echo "Artifacts:"
[[ -f "${SCRIPT_DIR}/layer1-errors/errors.json" ]] && echo "  Layer 1 errors:    tests/ax/layer1-errors/errors.json"
[[ -f "${SCRIPT_DIR}/layer1-errors/judgments.json" ]] && echo "  Layer 1 judgments: tests/ax/layer1-errors/judgments.json"
[[ -f "${SCRIPT_DIR}/layer2-tasks/results.json" ]] && echo "  Layer 2 results:   tests/ax/layer2-tasks/results.json"
[[ -f "${SCRIPT_DIR}/layer3-analysis/analysis.json" ]] && echo "  Layer 3 analysis:  tests/ax/layer3-analysis/analysis.json"
[[ -f "${SCRIPT_DIR}/layer3-analysis/analysis.md" ]] && echo "  Layer 3 report:    tests/ax/layer3-analysis/analysis.md"
echo ""

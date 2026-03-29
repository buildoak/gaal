#!/usr/bin/env bash
# analyze-traces.sh — Analyze Layer 2 agent task results for AX insights
#
# Reads Layer 2 results and Layer 1 judgments, dispatches a single Codex gpt-5.4
# xhigh worker to produce convergence, variance, error recovery, implicit API,
# and cost efficiency analysis.
#
# Usage:
#   ./analyze-traces.sh                                    # default paths
#   ./analyze-traces.sh --results PATH --judgments PATH    # custom inputs
#   ./analyze-traces.sh --dry-run                          # preview dispatch

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
AX_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
GAAL_ROOT="$(cd "$AX_DIR/../.." && pwd)"

RESULTS_FILE="${AX_DIR}/layer2-tasks/results.json"
JUDGMENTS_FILE="${AX_DIR}/layer1-errors/judgments.json"
OUTPUT_JSON="${SCRIPT_DIR}/analysis.json"
OUTPUT_MD="${SCRIPT_DIR}/analysis.md"
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --results) RESULTS_FILE="$2"; shift 2 ;;
        --judgments) JUDGMENTS_FILE="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        *) echo "Usage: $0 [--results PATH] [--judgments PATH] [--dry-run]" >&2; exit 1 ;;
    esac
done

if [[ ! -f "$RESULTS_FILE" ]]; then
    echo "Error: Layer 2 results not found at $RESULTS_FILE" >&2
    echo "Run layer2-tasks/run-tasks.sh first." >&2
    exit 1
fi

ANALYSIS_PROMPT=$(cat <<'PROMPT_EOF'
You are an AX (Agent Experience) researcher analyzing how AI agents interact with the gaal CLI tool for the first time.

## Input Data

You have been given:
1. **Layer 2 task results** (results.json): Multiple agents attempted various gaal tasks independently. Each entry includes the task prompt, the agent's response, dispatch metadata, and optionally session metadata from gaal.
2. **Layer 1 error judgments** (judgments.json, if available): Quality ratings for gaal's error messages.
3. **SKILL.md**: The documentation agents were given.

## Analysis Dimensions

Analyze the following dimensions and produce structured findings:

### 1. Convergence Analysis
- For each task, did all agents arrive at the same approach?
- Which commands showed highest convergence (agents independently chose the same solution)?
- Which showed divergence, and what explains the split?

### 2. Variance Analysis
- Where did agents scatter across different strategies?
- What factors correlated with strategy divergence (task ambiguity, doc gaps, flag confusion)?
- Rank tasks by variance (low variance = clear API, high variance = AX friction point)

### 3. Error Recovery
- When agents hit errors, did they self-correct?
- How many attempts did correction take?
- Which error messages led to successful recovery vs. spiraling?
- Cross-reference with Layer 1 judgments: do TEACHES-rated errors actually teach?

### 4. Implicit API
- What commands or flags did agents TRY that don't exist?
- What does this reveal about agent mental models?
- These are feature requests from the collective unconscious — rank by frequency

### 5. Cost Efficiency
- Tokens consumed per task (input + output)
- Attempts before success per task
- Cost per successful task completion
- Which tasks are cheapest/most expensive for agents?

### 6. AX Improvement Recommendations
- Top 5 concrete changes to gaal or its docs that would improve first-attempt success
- Each recommendation must cite specific evidence from the traces

## Output Format

Produce TWO outputs:

### analysis.json
```json
{
  "convergence": {
    "high_convergence_tasks": [...],
    "low_convergence_tasks": [...],
    "details": [{"task": "...", "convergence_score": 0.0-1.0, "dominant_approach": "...", "variants": [...]}]
  },
  "variance": {
    "ranked_by_variance": [{"task": "...", "variance_score": 0.0-1.0, "strategies_seen": [...]}]
  },
  "error_recovery": {
    "recovery_events": [{"task": "...", "agent": "...", "error_hit": "...", "recovered": true/false, "attempts": N}],
    "recovery_rate": 0.0-1.0
  },
  "implicit_api": {
    "phantom_commands": [{"command": "...", "frequency": N, "likely_intent": "..."}],
    "phantom_flags": [{"flag": "...", "on_command": "...", "frequency": N}]
  },
  "cost_efficiency": {
    "per_task": [{"task": "...", "avg_tokens": N, "avg_attempts": N, "success_rate": 0.0-1.0}],
    "total_tokens": N,
    "total_cost_estimate_usd": N
  },
  "recommendations": [
    {"priority": 1, "change": "...", "evidence": "...", "expected_impact": "..."}
  ]
}
```

### analysis.md
A human-readable report summarizing the findings with section headers, bullet points, and a final verdict on gaal's AX quality (A/B/C/D/F grade with justification).

Write both files to the current working directory as `analysis.json` and `analysis.md`.
PROMPT_EOF
)

echo "=== Layer 3: Trace Analysis ===" >&2
echo "Results: $RESULTS_FILE" >&2
echo "Judgments: $JUDGMENTS_FILE" >&2

if [[ "$DRY_RUN" == true ]]; then
    echo "" >&2
    echo "DRY RUN — would dispatch:" >&2
    echo "  Engine: codex" >&2
    echo "  Model: gpt-5.4" >&2
    echo "  Effort: xhigh" >&2
    echo "  Context files: $RESULTS_FILE, ${GAAL_ROOT}/skill/SKILL.md" >&2
    [[ -f "$JUDGMENTS_FILE" ]] && echo "  + $JUDGMENTS_FILE" >&2
    echo "  CWD: $SCRIPT_DIR" >&2
    exit 0
fi

# Build context-file args
context_args=(
    --context-file "$RESULTS_FILE"
    --context-file "${GAAL_ROOT}/skill/SKILL.md"
)
if [[ -f "$JUDGMENTS_FILE" ]]; then
    context_args+=(--context-file "$JUDGMENTS_FILE")
fi

# Dispatch to Codex gpt-5.4 xhigh via agent-mux
result=$(agent-mux \
    --engine codex \
    --model gpt-5.4 \
    --effort xhigh \
    --sandbox workspace-write \
    --cwd "$SCRIPT_DIR" \
    "${context_args[@]}" \
    "$ANALYSIS_PROMPT")

# The worker should write analysis.json and analysis.md to CWD
# But also capture the response as a fallback
response=$(echo "$result" | jq -r '.response // .text // .' 2>/dev/null || echo "$result")

# If the worker didn't write files, save the response
if [[ ! -f "$OUTPUT_JSON" ]]; then
    echo "$response" > "$OUTPUT_JSON"
    echo "Warning: Worker did not write analysis.json — saved response as fallback" >&2
fi

if [[ ! -f "$OUTPUT_MD" ]]; then
    echo "$response" > "$OUTPUT_MD"
    echo "Warning: Worker did not write analysis.md — saved response as fallback" >&2
fi

echo "" >&2
echo "Analysis complete." >&2
echo "  JSON: $OUTPUT_JSON" >&2
echo "  Report: $OUTPUT_MD" >&2

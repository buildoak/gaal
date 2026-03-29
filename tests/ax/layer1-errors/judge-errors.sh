#!/usr/bin/env bash
# judge-errors.sh — Dispatch error JSON to a Codex judge for AX quality rating
#
# Reads errors.json (Layer 1 output) and dispatches a single gpt-5.4 xhigh
# worker via agent-mux to evaluate every error message for agent learnability.
#
# Usage:
#   ./judge-errors.sh                           # uses default errors.json
#   ./judge-errors.sh --input path/to/errors.json
#   ./judge-errors.sh --dry-run                 # preview the dispatch, don't run

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
AX_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
GAAL_ROOT="$(cd "$AX_DIR/../.." && pwd)"
INPUT="${SCRIPT_DIR}/errors.json"
OUTPUT="${SCRIPT_DIR}/judgments.json"
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --input) INPUT="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        *) echo "Usage: $0 [--input PATH] [--output PATH] [--dry-run]" >&2; exit 1 ;;
    esac
done

if [[ ! -f "$INPUT" ]]; then
    echo "Error: errors.json not found at $INPUT" >&2
    echo "Run generate-errors.sh first: ./generate-errors.sh > errors.json" >&2
    exit 1
fi

JUDGE_PROMPT=$(cat <<'PROMPT_EOF'
You are evaluating CLI error messages for agent learnability. You are an expert in developer experience and agent experience (AX) design.

## Context

You have been given:
1. A JSON file containing error outputs from the `gaal` CLI — each entry has the command that was run, the exit code, stdout, and stderr.
2. The SKILL.md file that describes what agents see when they use gaal.

## Your Task

For EACH error entry in the JSON, evaluate the error message quality using this rubric:

### Rating Scale

- **TEACHES** — The error message tells the agent exactly what went wrong, gives a working example it can copy-paste, and names what to try next. An agent reading this error can self-correct on the first attempt.
- **HINTS** — The error message indicates the general problem area but is missing one or more of: a specific diagnosis, a copyable example, or actionable next-step guidance. The agent might need 2-3 attempts to recover.
- **CRYPTIC** — The error message is generic, technical jargon only, or provides no guidance on how to fix the problem. The agent will likely spiral or give up.

### Evaluation Criteria

For each error, check:
1. Does the error include a **specific problem statement** (not just "error occurred")?
2. Does the error include a **working example** the agent can copy?
3. Does the error **name the valid options** when the input was invalid?
4. Does the error include a **hint** for what to do next?
5. Is the exit code **meaningful** and consistent with the documented codes?

### Output Format

Return a JSON array. For each error entry, produce:

```json
{
  "name": "<error name from input>",
  "command": "<the command that was run>",
  "exit_code": <actual exit code>,
  "rating": "TEACHES|HINTS|CRYPTIC",
  "reasoning": "<1-2 sentence explanation of the rating>",
  "has_example": true|false,
  "has_valid_options": true|false,
  "has_hint": true|false,
  "proposed_fix": "<if HINTS or CRYPTIC: the exact error message text that would upgrade this to TEACHES. null if already TEACHES>"
}
```

### Important

- Be strict. TEACHES means genuinely self-correcting for an LLM agent, not just "has some info."
- For entries where expected_exit != actual_exit (exit_match: false), flag this as a bug regardless of message quality.
- Success cases (exit 0) should be rated on whether the success output is clear and useful, not on error quality.
- The judge output must be valid JSON. No markdown wrapping, no explanation outside the array.
PROMPT_EOF
)

echo "=== Layer 1: Error Quality Judge ===" >&2
echo "Input: $INPUT" >&2
echo "Output: $OUTPUT" >&2

if [[ "$DRY_RUN" == true ]]; then
    echo "" >&2
    echo "DRY RUN — would dispatch:" >&2
    echo "  Engine: codex" >&2
    echo "  Model: gpt-5.4" >&2
    echo "  Effort: xhigh" >&2
    echo "  Context files: $INPUT, ${GAAL_ROOT}/skill/SKILL.md" >&2
    echo "  CWD: $GAAL_ROOT" >&2
    echo "  Prompt length: ${#JUDGE_PROMPT} chars" >&2
    exit 0
fi

# Dispatch to Codex gpt-5.4 xhigh via agent-mux
result=$(agent-mux \
    --engine codex \
    --model gpt-5.4 \
    --effort xhigh \
    --sandbox workspace-write \
    --cwd "$GAAL_ROOT" \
    --context-file "$INPUT" \
    --context-file "${GAAL_ROOT}/skill/SKILL.md" \
    "$JUDGE_PROMPT")

# Extract the response and write to output
echo "$result" | jq -r '.response // .text // .' > "$OUTPUT"

echo "" >&2
echo "Judgments written to: $OUTPUT" >&2

# Quick summary
if command -v jq &>/dev/null && [[ -f "$OUTPUT" ]]; then
    teaches=$(jq '[.[] | select(.rating == "TEACHES")] | length' "$OUTPUT" 2>/dev/null || echo "?")
    hints=$(jq '[.[] | select(.rating == "HINTS")] | length' "$OUTPUT" 2>/dev/null || echo "?")
    cryptic=$(jq '[.[] | select(.rating == "CRYPTIC")] | length' "$OUTPUT" 2>/dev/null || echo "?")
    echo "Summary: TEACHES=$teaches  HINTS=$hints  CRYPTIC=$cryptic" >&2
fi

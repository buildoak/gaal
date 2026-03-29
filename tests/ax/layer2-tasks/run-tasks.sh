#!/usr/bin/env bash
# run-tasks.sh — Dispatch first-attempt agent tasks and collect results
#
# For each task in tasks.toml, dispatches N Codex gpt-5.4-mini-high workers
# with ONLY skill/SKILL.md and docs/ as context. Collects results, traces,
# and session metadata via gaal.
#
# Usage:
#   ./run-tasks.sh                      # run all tasks
#   ./run-tasks.sh --dry-run            # preview dispatches without running
#   ./run-tasks.sh --task find-file-author  # run a single task
#   ./run-tasks.sh --agents 1           # override agents_per_task

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
AX_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
GAAL_ROOT="$(cd "$AX_DIR/../.." && pwd)"
TASKS_FILE="${SCRIPT_DIR}/tasks.toml"
OUTPUT="${SCRIPT_DIR}/results.json"
DRY_RUN=false
SINGLE_TASK=""
AGENT_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --task) SINGLE_TASK="$2"; shift 2 ;;
        --agents) AGENT_OVERRIDE="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        *) echo "Usage: $0 [--dry-run] [--task NAME] [--agents N] [--output PATH]" >&2; exit 1 ;;
    esac
done

if [[ ! -f "$TASKS_FILE" ]]; then
    echo "Error: tasks.toml not found at $TASKS_FILE" >&2
    exit 1
fi

SYSTEM_PROMPT=$(cat <<'SYSPROMPT_EOF'
You are a software engineer using the gaal CLI for the first time. You have access to the gaal skill file (SKILL.md) and documentation (docs/) as context.

Your job is to accomplish the task given to you by running gaal commands. Think step by step:
1. Read the skill file and docs to understand what commands are available
2. Choose the right command and flags
3. Run the command
4. Interpret the output

Important rules:
- Use gaal commands only — do not grep JSONL files directly
- Prefer JSON output (default) over human output for machine-readable results
- If a command fails, read the error message carefully and try to self-correct
- Report your final answer clearly, including the exact commands you ran
SYSPROMPT_EOF
)

# Parse tasks.toml — extract task entries
parse_tasks() {
    local name="" prompt="" expected_command_pattern="" agents_per_task="" category=""
    local in_entry=false

    while IFS= read -r line; do
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        [[ -z "${line// /}" ]] && continue

        if [[ "$line" =~ ^\[\[tasks\]\] ]]; then
            if [[ -n "$name" && -n "$prompt" ]]; then
                local agents="${agents_per_task:-3}"
                [[ -n "$AGENT_OVERRIDE" ]] && agents="$AGENT_OVERRIDE"
                printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$prompt" "$expected_command_pattern" "$agents" "$category"
            fi
            name="" prompt="" expected_command_pattern="" agents_per_task="" category=""
            in_entry=true
            continue
        fi

        if [[ "$in_entry" == true ]]; then
            if [[ "$line" =~ ^name[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                name="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^prompt[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                prompt="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^expected_command_pattern[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                expected_command_pattern="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^agents_per_task[[:space:]]*=[[:space:]]*([0-9]+) ]]; then
                agents_per_task="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^category[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                category="${BASH_REMATCH[1]}"
            fi
        fi
    done < "$TASKS_FILE"

    # Emit last entry
    if [[ -n "$name" && -n "$prompt" ]]; then
        local agents="${agents_per_task:-3}"
        [[ -n "$AGENT_OVERRIDE" ]] && agents="$AGENT_OVERRIDE"
        printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$prompt" "$expected_command_pattern" "$agents" "$category"
    fi
}

# Export GAAL_HOME so dispatched workers can find the gaal database even in
# sandboxed environments where ~/.gaal/ is not writable.
if [[ -z "${GAAL_HOME:-}" ]]; then
    export GAAL_HOME="${HOME}/.gaal"
fi

echo "=== Layer 2: First-Attempt Agent Tasks ===" >&2
echo "Tasks file: $TASKS_FILE" >&2
echo "GAAL_HOME: $GAAL_HOME" >&2
echo "Output: $OUTPUT" >&2

all_results="[]"
task_count=0
dispatch_count=0

while IFS=$'\t' read -r name prompt expected_pattern agents category; do
    # Filter to single task if specified
    if [[ -n "$SINGLE_TASK" && "$name" != "$SINGLE_TASK" ]]; then
        continue
    fi

    task_count=$((task_count + 1))
    echo "" >&2
    echo "--- Task: $name (category: $category, agents: $agents) ---" >&2

    task_results="[]"

    for i in $(seq 1 "$agents"); do
        dispatch_count=$((dispatch_count + 1))
        agent_label="${name}-agent${i}"

        echo "  Dispatching $agent_label..." >&2

        if [[ "$DRY_RUN" == true ]]; then
            echo "  [DRY RUN] Would dispatch:" >&2
            echo "    Engine: codex, Model: gpt-5.4-mini, Effort: high" >&2
            echo "    Prompt: ${prompt:0:80}..." >&2
            continue
        fi

        # Dispatch via agent-mux
        dispatch_start=$(date +%s)

        set +e
        result=$(agent-mux \
            --engine codex \
            --model gpt-5.4-mini \
            --effort high \
            --sandbox none \
            --cwd "$GAAL_ROOT" \
            --context-file "${GAAL_ROOT}/skill/SKILL.md" \
            --context-file "${GAAL_ROOT}/docs/README.md" \
            --system-prompt "$SYSTEM_PROMPT" \
            "$prompt" 2>/dev/null)
        dispatch_exit=$?
        set -e

        dispatch_end=$(date +%s)
        dispatch_duration=$((dispatch_end - dispatch_start))

        # Extract dispatch ID from result if available
        dispatch_id=$(echo "$result" | jq -r '.dispatch_id // .id // empty' 2>/dev/null || echo "")
        response=$(echo "$result" | jq -r '.response // .text // .' 2>/dev/null || echo "$result")

        # Try to find the worker's session via gaal
        session_data="{}"
        if [[ -n "$dispatch_id" ]]; then
            set +e
            salt_search=$(gaal search "AGENT_MUX_GO_${dispatch_id}" --limit 1 2>/dev/null)
            worker_session_id=$(echo "$salt_search" | jq -r '.results[0].session_id // empty' 2>/dev/null || echo "")
            if [[ -n "$worker_session_id" ]]; then
                session_data=$(gaal inspect "$worker_session_id" --tokens --json 2>/dev/null || echo "{}")
            fi
            set -e
        fi

        # Build agent result entry
        agent_entry=$(jq -n \
            --arg agent_label "$agent_label" \
            --arg task_name "$name" \
            --arg dispatch_id "$dispatch_id" \
            --argjson dispatch_exit "$dispatch_exit" \
            --argjson dispatch_duration "$dispatch_duration" \
            --arg response "$response" \
            --arg expected_pattern "$expected_pattern" \
            --argjson session_data "$session_data" \
            '{
                agent_label: $agent_label,
                task_name: $task_name,
                dispatch_id: $dispatch_id,
                dispatch_exit: $dispatch_exit,
                dispatch_duration_secs: $dispatch_duration,
                response: $response,
                expected_pattern: $expected_pattern,
                session_data: $session_data
            }')

        task_results=$(echo "$task_results" | jq --argjson entry "$agent_entry" '. + [$entry]')
    done

    # Wrap task
    task_entry=$(echo "$task_results" | jq \
        --arg name "$name" \
        --arg category "$category" \
        --arg prompt "$prompt" \
        --arg expected_pattern "$expected_pattern" \
        --argjson agents "$agents" \
        '{
            task_name: $name,
            category: $category,
            prompt: $prompt,
            expected_pattern: $expected_pattern,
            agents_dispatched: $agents,
            agent_results: .
        }')

    all_results=$(echo "$all_results" | jq --argjson entry "$task_entry" '. + [$entry]')

done < <(parse_tasks)

# Wrap in envelope
envelope=$(echo "$all_results" | jq \
    --argjson task_count "$task_count" \
    --argjson dispatch_count "$dispatch_count" \
    --arg timestamp "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{
        meta: {
            timestamp: $timestamp,
            tasks_run: $task_count,
            total_dispatches: $dispatch_count
        },
        tasks: .
    }')

if [[ "$DRY_RUN" == true ]]; then
    echo "" >&2
    echo "DRY RUN complete. Would dispatch $dispatch_count workers across $task_count tasks." >&2
else
    echo "$envelope" | jq '.' > "$OUTPUT"
    echo "" >&2
    echo "Results written to: $OUTPUT" >&2
    echo "Tasks: $task_count | Dispatches: $dispatch_count" >&2
fi

#!/usr/bin/env bash
# generate-errors.sh — Trigger every known gaal error path and collect outputs
#
# Reads error-manifest.toml for the declarative list of commands.
# Outputs a JSON array: [{name, command, expected_exit, actual_exit, stdout, stderr}]
#
# Usage:
#   ./generate-errors.sh > errors.json
#   ./generate-errors.sh --pretty          # jq-formatted output
#   ./generate-errors.sh --manifest PATH   # custom manifest path

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="${SCRIPT_DIR}/error-manifest.toml"
PRETTY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pretty) PRETTY=true; shift ;;
        --manifest) MANIFEST="$2"; shift 2 ;;
        *) echo "Usage: $0 [--pretty] [--manifest PATH]" >&2; exit 1 ;;
    esac
done

if [[ ! -f "$MANIFEST" ]]; then
    echo "Error: manifest not found at $MANIFEST" >&2
    exit 1
fi

# Parse TOML manifest — extract name, command, expected_exit, description
# We use a simple state-machine parser since we don't want external deps
parse_manifest() {
    local name="" command="" expected_exit="" description=""
    local in_entry=false

    while IFS= read -r line; do
        # Skip comments and blank lines
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        [[ -z "${line// /}" ]] && continue

        # New entry
        if [[ "$line" =~ ^\[\[errors\]\] ]]; then
            # Emit previous entry if we had one
            if [[ -n "$name" && -n "$command" ]]; then
                printf '%s\t%s\t%s\t%s\n' "$name" "$command" "$expected_exit" "$description"
            fi
            name="" command="" expected_exit="" description=""
            in_entry=true
            continue
        fi

        if [[ "$in_entry" == true ]]; then
            if [[ "$line" =~ ^name[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                name="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^command[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                command="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^expected_exit[[:space:]]*=[[:space:]]*([0-9]+) ]]; then
                expected_exit="${BASH_REMATCH[1]}"
            elif [[ "$line" =~ ^description[[:space:]]*=[[:space:]]*\"(.*)\" ]]; then
                description="${BASH_REMATCH[1]}"
            fi
        fi
    done < "$MANIFEST"

    # Emit last entry
    if [[ -n "$name" && -n "$command" ]]; then
        printf '%s\t%s\t%s\t%s\n' "$name" "$command" "$expected_exit" "$description"
    fi
}

# Run each command and collect results
results="[]"
total=0
pass=0
fail=0

while IFS=$'\t' read -r name command expected_exit description; do
    total=$((total + 1))

    # Capture stdout, stderr, and exit code
    stdout_file=$(mktemp)
    stderr_file=$(mktemp)

    set +e
    eval "$command" > "$stdout_file" 2> "$stderr_file"
    actual_exit=$?
    set -e

    stdout_content=$(cat "$stdout_file")
    stderr_content=$(cat "$stderr_file")
    rm -f "$stdout_file" "$stderr_file"

    # Check if exit code matches
    exit_match=true
    if [[ "$actual_exit" -ne "$expected_exit" ]]; then
        exit_match=false
        fail=$((fail + 1))
        echo "MISMATCH: $name — expected exit $expected_exit, got $actual_exit" >&2
    else
        pass=$((pass + 1))
    fi

    # Build JSON entry — use jq for proper escaping
    entry=$(jq -n \
        --arg name "$name" \
        --arg command "$command" \
        --arg description "$description" \
        --argjson expected_exit "$expected_exit" \
        --argjson actual_exit "$actual_exit" \
        --argjson exit_match "$exit_match" \
        --arg stdout "$stdout_content" \
        --arg stderr "$stderr_content" \
        '{
            name: $name,
            command: $command,
            description: $description,
            expected_exit: $expected_exit,
            actual_exit: $actual_exit,
            exit_match: $exit_match,
            stdout: $stdout,
            stderr: $stderr
        }')

    results=$(echo "$results" | jq --argjson entry "$entry" '. + [$entry]')

done < <(parse_manifest)

# Wrap in envelope
envelope=$(echo "$results" | jq \
    --argjson total "$total" \
    --argjson pass "$pass" \
    --argjson fail "$fail" \
    --arg timestamp "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg manifest "$MANIFEST" \
    '{
        meta: {
            timestamp: $timestamp,
            manifest: $manifest,
            total: $total,
            pass: $pass,
            fail: $fail
        },
        errors: .
    }')

if [[ "$PRETTY" == true ]]; then
    echo "$envelope" | jq '.'
else
    echo "$envelope"
fi

# Summary to stderr
echo "" >&2
echo "=== Error Generation Summary ===" >&2
echo "Total: $total | Pass: $pass | Fail (exit mismatch): $fail" >&2

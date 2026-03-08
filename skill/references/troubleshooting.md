# Troubleshooting — gaal

## Known Issues

| Issue | Symptom | Cause | Workaround |
|-------|---------|-------|------------|
| **Pipe gotcha with `gaal who`** | `gaal who wrote X \| jq` fails or produces unexpected output | `who` verb consumes trailing arguments greedily, eating the pipe | Capture to variable first: `OUTPUT=$(gaal who wrote X); echo "$OUTPUT" \| jq` |
| **`-H` on `active` renders JSON** | `gaal active -H` may output JSON instead of table | Table formatter not fully wired for active verb | Use default JSON output and pipe through `jq` or `column -t` |
| **`recall` returns exit 1 with data** | `gaal recall "topic"` exits 1 even when sessions exist | Handoffs table is empty (no `gaal handoff` runs yet) | Run `gaal handoff today` first to populate handoffs, then retry recall |
| **`contains_error` false positives** | Sessions flagged with errors that have none | Strings like `"error_count: 0"` or `"no errors found"` match error detection | Check `.errors` array in `gaal show <id> --errors` for actual errors. Ignore summary-level flags. |
| **Claude token counts near-zero** | Claude sessions show `tokens.input: 13, tokens.output: 11` despite hundreds of tool calls | Claude JSONL parser not accumulating `usage.input_tokens`/`usage.output_tokens` from assistant message records | Known bug (TESTING.md T-20). Use `tools_used` as a proxy for session activity. Codex sessions unaffected. |
| **CWD collision in `active`** | Same session ID appears multiple times in `gaal active` output | Multiple PIDs (parent + subagent children) share same CWD and JSONL path | Known bug (TESTING.md T-19). Deduplicate by session ID when processing results: `gaal active \| jq 'unique_by(.id)'` |
| **Epoch-dated eywa stubs** | `gaal index status` shows `oldest_session: "1970-01-01T00:00:00Z"` | `import-eywa` creates session stubs with epoch timestamp when date is missing | Known bug (TESTING.md T-18). Cosmetic — does not affect queries. Filter with `--since` to exclude. |
| **Inflated active count in index** | `sessions_by_status.active` is much higher than actual running processes | Sessions with `ended_at IS NULL` classified as "active" even when long-dead | Known bug (TESTING.md T-BONUS-04). Use `gaal active` for accurate live count. `ls --status active` queries the (possibly stale) archive. |

## Quick Diagnostics

```bash
# Check if index exists and has data
gaal index status | jq '{sessions: .sessions_total, facts: .facts_total, handoffs: .handoffs_total}'

# Check if handoffs exist (required for recall)
gaal index status | jq '.handoffs_total'

# Check if active sessions are discoverable
gaal active 2>/dev/null && echo "Active sessions found" || echo "No active sessions (or discovery failed)"

# Verify binary version
gaal --version
```

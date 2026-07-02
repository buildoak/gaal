# Troubleshooting — gaal

## Known Issues

| Issue | Symptom | Cause | Workaround |
|-------|---------|-------|------------|
| **`recall` returns no useful results** | `gaal recall "topic"` returns no matches even when sessions exist | Recall searches generated handoffs, not raw session facts | Use `gaal search` for raw indexed facts, or generate a handoff for a chosen session only when continuity generation is appropriate |
| **`contains_error` false positives** | Sessions flagged with errors that have none | Strings like `"error_count: 0"` or `"no errors found"` match error detection | Check `.errors` array in `gaal inspect <id> --errors` for actual errors. Ignore summary-level flags. |
| **Claude token counts near-zero** | Claude sessions show `tokens.input: 13, tokens.output: 11` despite hundreds of tool calls | Claude JSONL parser did not accumulate usage from some historical assistant message shapes | Use `tools_used` as a proxy for old sessions and reindex after parser fixes. Codex sessions unaffected. |
| **Backfill cursor stuck** | `gaal index backfill` keeps skipping new or modified sessions | Per-engine mtime cursor in `meta` table is ahead of truth (e.g., after a crash or manual file-mtime edit) | Force full rescan: `sqlite3 ~/.gaal/index.db "DELETE FROM meta WHERE key LIKE 'backfill:%'"` then re-run `gaal index backfill`. Safe — only clears cursors, not indexed rows. |

## Quick Diagnostics

```bash
# Check if index exists and has data
gaal index status | jq '{sessions: .sessions_total, facts: .facts_total, handoffs: .handoffs_total}'

# Check if handoffs exist (required for recall)
gaal index status | jq '.handoffs_total'

# Verify binary version
gaal --version
```

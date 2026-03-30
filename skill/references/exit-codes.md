# Exit Codes — gaal

| Code | Name | When it fires | Agent response |
|------|------|--------------|----------------|
| 0 | Success | Command completed, output produced | Parse JSON from stdout |
| 1 | NoResults | Query matched nothing (empty result set) | Valid state. Broaden filters (`--since`, `--limit`) or check if index has data (`gaal index status`) |
| 2 | Ambiguous | ID prefix matches 2+ sessions | Provide more characters of the session ID, or use `gaal ls` to find the exact ID |
| 3 | NotFound | Session ID does not exist in index | Verify ID is correct. May need `gaal index backfill` if session is recent |
| 10 | NoIndex | `~/.gaal/index.db` does not exist or has no schema | Run `gaal index backfill` to create and populate the index |
| 11 | ParseError | Invalid input (bad verb, malformed flag, unparseable date) | Fix the command. Check verb spelling (`who` verbs: read, wrote, ran, touched, changed, deleted) |

## Stderr Format

On non-zero exit, stderr contains a full JSON error envelope:
```json
{"ok": false, "error": "descriptive message", "hint": "what to try next", "example": "gaal <correct invocation>", "exit_code": N}
```

All five fields are always present: `ok` (always `false`), `error`, `hint`, `example`, and `exit_code`.

## Script Pattern

```bash
OUTPUT=$(gaal ls --limit 5 2>/dev/null)
EXIT=$?
case $EXIT in
  0) echo "$OUTPUT" | jq '.' ;;
  1) echo "No results" ;;
  10) echo "Run: gaal index backfill" ;;
  *) echo "Error (exit $EXIT)" ;;
esac
```

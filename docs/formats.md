# Formats Reference

This page describes the output shapes, mode switches, and exit codes that agents and humans should expect from `gaal`.

## Output Modes

### JSON (default)

Most commands emit JSON by default. Exceptions:

- `salt` prints a raw token string
- `recall` prints help and exits successfully when no query is given
- `transcript` prints markdown when `--stdout` is used

### Human Mode (`-H` / `--human`)

Most commands accept `-H` and switch to table or card output.

## recall --format Comparison

Use `gaal recall ... --format <name>` to control how much handoff material is returned.

| Format | What it returns | When to use | Token cost |
|--------|----------------|-------------|------------|
| `brief` (default) | Compact session summary blocks | Agent retrieval, quick overview | Low (~500 tokens) |
| `summary` | Structured fields only (`headline`, `projects`, `keywords`, `substance`) | Programmatic parsing, comparison | Low |
| `handoff` | Structured summary + raw handoff content | Full context recovery | Medium |
| `full` | Summary + handoff + files + errors | Deep investigation | High |
| `eywa` | Legacy markdown-oriented layout | Backwards compatibility with eywa consumers | Medium |

## JSON Error Format

All errors include these fields:

```json
{"ok": false, "error": "...", "hint": "...", "example": "...", "exit_code": N}
```

Human mode (`-H`) routes through `format_human()` which renders:

- What went wrong: `<specific problem>`
- Example: `<correct invocation>`
- Hint: `<what to try next>`

## Exit Code Reference

| Code | Meaning | Agent response |
|------|---------|----------------|
| `0` | Success | Process output normally |
| `1` | No results | Widen search/filter parameters |
| `2` | Ambiguous ID | Provide a longer ID prefix |
| `3` | Not found | Verify the ID exists with `gaal ls` |
| `10` | Missing index | Run `gaal index backfill` |
| `11` | Parse error | Check input format |

## inspect Output Shapes

Default behavior:

- Compact session card with counts, token summary, tags, and `session_type`

Focused flags:

- `--files`, `--commands`, `--errors`, `--git`, `--trace` swap in specific payloads

Batch mode:

- `--ids`, `--tag` return an array

Human mode:

- Card view with subagent table for coordinators

## ls Output Envelope

Default response fields:

- `query_window`
- `filter`
- `shown`
- `total`
- `total_unfiltered`
- `sessions` array

Aggregate mode returns totals and engine buckets instead of session rows.

## Time Filters

Most query commands accept:

- Relative: `1h`, `7d`, `2w`, `today`
- Absolute dates: `2026-03-29`
- Timestamps: RFC3339 or `YYYY-MM-DDTHH:MM`

## Noise Filtering

By default `ls` hides sessions with zero tool calls and duration under 30s. Use `--all` to include.

## Search Index Rebuild Triggers

These commands rebuild Tantivy:

- `index backfill`
- `index reindex`
- `index prune`
- `index import-eywa`

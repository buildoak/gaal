# Verb Reference — gaal

Verified flag reference for the currently installed `gaal` binary.

The goal here is accuracy over exhaustiveness. If a flag or output shape is
not listed below, do not assume it exists just because an older doc mentioned
it.

---

## Commands

Current top-level commands:

1. `ls` — fleet view across indexed sessions
2. `inspect` — session details with focused views; **formerly `show`**
3. `transcript` — rendered transcript markdown path or stdout dump
4. `who` — inverted queries
5. `search` — full-text search over indexed facts
6. `recall` — semantic session retrieval
7. `salt` — self-identification token
8. `find-salt` — JSONL discovery by salt
9. `create-handoff` — LLM extraction into handoff markdown
10. `index` — index maintenance
11. `tag` — apply/remove tags

There is **no separate `active` command** in the current binary.

---

## 1. `ls`

Fleet view across sessions.

### Flags

| Flag | Meaning |
|------|---------|
| `--engine <ENGINE>` | Filter by `claude` or `codex` |
| `--since <SINCE>` | Lower bound; supports durations/dates such as `1d` or `2026-03-01` |
| `--before <BEFORE>` | Upper bound date/time |
| `--cwd <CWD>` | Restrict by working directory substring |
| `--tag <TAG>` | Restrict by tag (repeatable; AND logic) |
| `--sort <SORT>` | `started`, `ended`, `tokens`, `cost`, or `duration` |
| `--limit <LIMIT>` | Max number of results |
| `--aggregate` | Return totals instead of individual sessions |
| `--all` | Include noisy sessions (0 tool calls and <30s duration) |
| `-H, --human` | Human-readable output |

### Notes

- Default JSON output is an object with session metadata plus a `sessions` array.
- In normal mode `gaal ls` hides noise; use `--all` to see everything.
- Use `--aggregate` for token totals and grouped counts.

### Examples

```bash
gaal ls --engine claude --since 3d --limit 5 -H
gaal ls --since 2026-03-20 --before 2026-03-21 --all
gaal ls --aggregate --since 7d
```

---

## 2. `inspect`

Session details with optional focused views. This is the command that replaced
older `show` docs.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Session ID, unique prefix, or `latest` |
| `--files [read\|write\|all]` | File-ops view; bare `--files` defaults to `all` |
| `--errors` | Errors and non-zero exits only |
| `--commands` | Commands only |
| `--git` | Git operations only |
| `--tokens` | Token usage breakdown |
| `--trace` | Full event timeline |
| `--source` | Raw JSONL source path |
| `--ids <IDS>` | Batch IDs in comma-delimited form |
| `--tag <TAG>` | Batch filter by tag |
| `-F, --full` | Include full arrays and detail fields |
| `-H, --human` | Human-readable output |

### Notes

- `gaal inspect latest` returns a compact operational/session summary in JSON.
- Focus flags such as `--files`, `--errors`, or `--commands` narrow the output.
- `gaal inspect --ids ...` is the batch-friendly replacement for looping over
  old `show` calls.

### Examples

```bash
gaal inspect latest
gaal inspect latest --files write
gaal inspect 249aad1e --trace
gaal inspect --ids a1b2c3d4,e5f6g7h8 --files read
```

---

## 3. `transcript`

Rendered transcript markdown access. This replaced older `inspect --markdown`
style behavior.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Session ID, unique prefix, or `latest` |
| `--force` | Re-render even if cached markdown exists |
| `--stdout` | Dump markdown to stdout instead of returning path metadata |
| `-H, --human` | Human-readable output |

### Notes

- Default behavior is **path-first**: JSON with transcript path, size, and
  estimated token count.
- Use `--stdout` only when you explicitly want the markdown content in the
  current calling context.

### Examples

```bash
gaal transcript latest
gaal transcript 249aad1e
gaal transcript latest --stdout
```

---

## 4. `who`

Inverted query: which session did X to Y?

### Verbs

| Verb | Meaning |
|------|---------|
| `read` | File read operations |
| `wrote` | File writes/edits |
| `ran` | Command executions |
| `touched` | Broadest interaction query |
| `changed` | File modifications |
| `deleted` | File deletions or removal commands |

There is **no `installed` verb** in the current binary.

### Flags

| Flag | Meaning |
|------|---------|
| `[VERB]` | One of the verbs above |
| `[TARGET]` | File path, command pattern, or search term |
| `--since <SINCE>` | Lower bound time window |
| `--before <BEFORE>` | Upper bound date/time |
| `--cwd <CWD>` | Restrict by working directory |
| `--engine <ENGINE>` | Restrict by engine |
| `--tag <TAG>` | Restrict by tag |
| `--failed` | For `ran`, show only failed commands |
| `--limit <LIMIT>` | Max number of results |
| `-F, --full` | Full per-fact output |
| `-H, --human` | Human-readable output |

### Example

```bash
OUTPUT=$(gaal who wrote CLAUDE.md --since 7d)
echo "$OUTPUT" | jq '.'
```

---

## 5. `search`

Full-text search over indexed facts.

### Flags

| Flag | Meaning |
|------|---------|
| `[QUERY]` | Search query |
| `--since <SINCE>` | Lower bound time window |
| `--cwd <CWD>` | Restrict by working directory |
| `--engine <ENGINE>` | Restrict by engine |
| `--field <FIELD>` | `prompts`, `replies`, `commands`, `errors`, `files`, or `all` |
| `--context <CONTEXT>` | Context lines around each match |
| `--limit <LIMIT>` | Max number of results |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal search "gaussian moat" --field commands --limit 5 -H
```

---

## 6. `recall`

Semantic session retrieval. This is the eywa replacement surface.

### Flags

| Flag | Meaning |
|------|---------|
| `[QUERY]` | Optional topic query |
| `--days-back <DAYS_BACK>` | Recency window in days |
| `--limit <LIMIT>` | Max number of sessions |
| `--format <FORMAT>` | `summary`, `handoff`, `brief`, `full`, or `eywa` |
| `--substance <SUBSTANCE>` | Minimum substance score |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal recall "peekaboo" --format brief --limit 5
gaal recall --format eywa
```

---

## 7. `create-handoff`

LLM-powered handoff generation.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Session ID or `today` |
| `--jsonl <JSONL>` | Explicit JSONL path |
| `--engine <ENGINE>` | Extraction engine: `claude` or `codex` |
| `--model <MODEL>` | Extraction model |
| `--prompt <PROMPT>` | Custom prompt path |
| `--provider <PROVIDER>` | `agent-mux` or `openrouter` |
| `--format <FORMAT>` | Output format; default is `eywa-compatible` |
| `--batch` | Batch mode |
| `--since <SINCE>` | Lower bound for batch candidates |
| `--parallel <PARALLEL>` | Max concurrent batch workers |
| `--min-turns <MIN_TURNS>` | Minimum turns required for batch candidates |
| `--this` | Prefer nearest detected session over parent session |
| `--dry-run` | Preview candidates without processing |
| `-H, --human` | Human-readable output |

### Examples

```bash
gaal create-handoff 249aad1e
gaal create-handoff --batch --since 1d --dry-run
gaal create-handoff --jsonl "$JSONL"
```

---

## 8. `index`

Index maintenance commands.

### Subcommands

| Subcommand | Meaning |
|------------|---------|
| `backfill` | Index all existing JSONL files |
| `status` | Show index health/status |
| `reindex` | Force re-index of one session |
| `import-eywa` | Import legacy eywa handoff-index data |
| `prune` | Remove old facts before a date |

### Example

```bash
gaal index status -H
gaal index backfill
gaal index reindex 249aad1e
```

---

## 9. `tag`

Apply or remove tags on a session.

### Flags

| Flag | Meaning |
|------|---------|
| `[ID]` | Session ID, or `ls` to list tags |
| `[TAGS]...` | Tags to add/remove |
| `--remove` | Remove tags instead of adding them |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal tag 249aad1e "research"
gaal tag 249aad1e --remove "research"
gaal tag ls
```

---

## 10. `salt`

Generate a random salt token for self-identification.

### Example

```bash
gaal salt
```

---

## 11. `find-salt`

Find the first JSONL file containing the provided salt token.

### Flags

| Flag | Meaning |
|------|---------|
| `[SALT]` | Salt token to search for |
| `-H, --human` | Human-readable output |

### Example

```bash
gaal find-salt "$SALT"
```

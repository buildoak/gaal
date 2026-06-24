---
name: gaal
description: |
  Public agent guide for Gaal, the local session observability CLI for AI coding agents.
  Use on first launch, before session-history work, when resuming context, attributing
  file or command activity, inspecting transcripts, searching traces, or creating
  optional continuity handoffs. Covers setup, safe local-trace handling, JSON-first
  command use, self-identification, and handoff generation requirements.
---

# gaal

Gaal turns AI coding-agent session traces into queryable artifacts. It indexes
local Claude Code, Codex, Gemini CLI, native Antigravity CLI (`agy`), and
experimental Hermes session stores,
then exposes one CLI for fleet views, inspection, attribution, full-text search,
recall, transcript rendering, self-identification, and optional handoff creation.

Treat this skill as required on first launch. A fresh agent should not guess
where session history lives or whether an index exists; it should run the setup
path below and then use Gaal's JSON output as the evidence surface.

## First Launch

Run this path before relying on Gaal in a new checkout, machine, container, or
agent harness:

```bash
gaal --version
gaal index status
```

If the binary is missing and you are inside a Gaal checkout:

```bash
cargo install --path .
```

If `gaal index status` reports no index, build the local index from existing
session traces:

```bash
gaal index backfill
gaal index status
gaal ls -H --limit 5
```

In a sandbox or CI environment, keep derived state in a writable throwaway
directory:

```bash
export GAAL_HOME=/tmp/gaal-home
gaal index backfill
gaal ls --limit 5
```

`GAAL_HOME` relocates Gaal's derived database, search index, config, rendered
transcripts, and generated handoffs. It does not relocate the source session
logs from the agent tools themselves.

## Safety Model

Gaal reads local agent traces. Those traces can contain private prompts, source
code, file paths, command output, credentials accidentally printed to terminals,
and tool results. Default to read-only commands until the task explicitly needs
index maintenance, tagging, or handoff generation.

Read-only commands:

```bash
gaal ls
gaal inspect latest
gaal transcript latest
gaal activity --since 1d
gaal who wrote path/to/file
gaal search "query"
gaal recall "topic"
gaal resolve latest
gaal salt
gaal find-salt GAAL_SALT_<hex>
gaal index status
```

Mutating or externally sensitive commands:

- `gaal index backfill`, `gaal index reindex`, `gaal index prune`,
  `gaal index import-eywa`, and `gaal index recover-orphans` write derived
  index state under `GAAL_HOME`.
- `gaal tag <id> <tag>` and `gaal tag <id> --remove <tag>` change local tags.
- `gaal create-handoff ...` can send session content to an external LLM backend
  and may consume subscription quota, API credits, metered usage, or local
  compute depending on the configured backend.

Before using `create-handoff`, confirm the user actually wants a continuity
artifact generated from the local trace. Use `--dry-run` before batch work.

## Command Map

Use the smallest command that answers the current question:

| Need | Command |
|------|---------|
| Recent sessions / fleet view | `gaal ls` |
| One session's structured details | `gaal inspect <id>` |
| Rendered transcript path or content | `gaal transcript <id>` |
| Activity across a time window | `gaal activity --since <window>` |
| Who read/wrote/ran something | `gaal who <verb> <target>` |
| Full-text search across facts | `gaal search <query>` |
| Continuity from generated handoffs | `gaal recall <query>` |
| Resolve an ID to paths and metadata | `gaal resolve <id>` |
| Identify the current agent session | `gaal salt` then `gaal find-salt <token>` |
| Create an optional continuity artifact | `gaal create-handoff <id>` |
| Maintain the derived index | `gaal index ...` |
| Annotate a session locally | `gaal tag <id> <tag>` |

Default output is JSON for normal query commands. Add `-H` or `--human` only
when a human-readable table or card is more useful than structured output.
Exceptions: `gaal salt` prints a raw token string, and CLI argument-parser
errors may be plain text before Gaal's JSON error formatter runs.

## Common Agent Workflows

### Start With Continuity

Use recall when a task depends on previous work. Recall searches generated
handoffs, not raw traces, so an empty result can mean no handoffs exist yet.

```bash
gaal recall "auth migration" --format brief --limit 5
gaal recall --id latest --format handoff
gaal index status
```

If recall is empty, use `gaal search` or `gaal who` for raw indexed facts, or
generate a handoff only when the task calls for it.

### Find Relevant Sessions

```bash
gaal ls --since 7d -H
gaal ls --engine codex --since 7d
gaal ls --cwd path-fragment --since 30d
gaal search "database migration" --field all --limit 10
gaal who wrote src/main.rs --since 30d
gaal who ran cargo --failed --limit 20
```

`who` verbs are `read`, `wrote`, `ran`, `touched`, `changed`, and `deleted`.
Because `who` consumes trailing arguments greedily, capture before piping:

```bash
OUTPUT=$(gaal who wrote src/main.rs --since 7d)
echo "$OUTPUT" | jq '.'
```

### Inspect Evidence

```bash
gaal inspect latest --tokens -H
gaal inspect latest --files write
gaal inspect 249aad1e --trace
gaal inspect --ids a1b2c3d4,e5f6g7h8 --files read
gaal transcript latest
gaal transcript latest --stdout
```

`gaal transcript <id>` returns path metadata by default. Use `--stdout` only
when the transcript content belongs in the current agent context.

### Self-Identify The Current Session

Use the salt protocol when an agent needs its own session ID or source JSONL.

Step 1, one tool call:

```bash
gaal salt
```

Step 2, a separate later tool call after the first result has been written into
the session log:

```bash
gaal find-salt GAAL_SALT_<hex>
```

Pass the literal token printed by `gaal salt`. Do not rely on shell variables
persisting across agent tool calls.

`find-salt` scans Claude Code, Codex, and Antigravity brain JSONL session logs.
It does not identify Gemini JSON or Hermes SQLite sessions. For agy, it matches
salt output in executed action records and ignores user prompt echoes. When the
session is indexed, output is enriched with model, cwd, session type, token
counts when known, transcript status, and handoff status; otherwise it still
returns the source path and `"indexed": false`.

To create a handoff from the identified source path, first inspect the returned
JSON and confirm generation is appropriate:

```bash
RESULT=$(gaal find-salt GAAL_SALT_<hex>)
echo "$RESULT" | jq '{session_id, engine, jsonl_path, indexed, handoff}'
```

Then, only when a handoff is needed:

```bash
JSONL=$(echo "$RESULT" | jq -r .jsonl_path)
gaal create-handoff --jsonl "$JSONL" --dry-run
gaal create-handoff --jsonl "$JSONL"
```

## Handoffs

Handoffs are one of Gaal's main continuity features: they turn a long local
session trace into a compact, indexed artifact that future agents can retrieve
with `gaal recall`. They are optional. Normal searching and inspection work
without them.

Important constraints:

- `create-handoff` uses an external LLM backend and may transmit transcript
  content outside the local machine.
- The default provider is `agent-mux`; install and configure
  `agent-mux` before relying on `create-handoff`.
- OpenRouter may appear as a provider selector in some builds; verify
  `gaal create-handoff --dry-run` and the command docs before relying on it for
  real execution.
- Batch generation can consume quota or credits quickly. Always run dry-run first:
  `gaal create-handoff --batch --since 1d --min-turns 3 --dry-run`.

Useful forms:

```bash
command -v agent-mux
gaal create-handoff 249aad1e --dry-run
gaal create-handoff 249aad1e
gaal create-handoff latest --provider agent-mux --effort medium
gaal create-handoff --jsonl /path/to/session.jsonl --dry-run
gaal create-handoff --batch --since 1d --min-turns 3 --dry-run
```

Session ID notes:

- Claude Code, Codex, Gemini, and Agy usually resolve by short unique prefixes.
- Codex IDs are stored as the last 8 hex characters of the UUID with dashes
  removed.
- Agy IDs are the first 8 characters of the Antigravity brain UUID; the full
  UUID remains recoverable from the source path.
- Hermes IDs are full logical session IDs; short date-like prefixes can collide.

## Engine Support

Main indexed engines:

- `claude`: Claude Code JSONL traces.
- `codex`: Codex JSONL traces.
- `gemini`: Gemini CLI JSON session files.
- `agy`: experimental native Antigravity CLI JSONL traces from
  `~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`,
  with fallback to `transcript.jsonl`.
- `hermes`: experimental Hermes Agent SQLite support.

Agy caveats: token/cost parity and SQLite/blob sidecars are out of scope; image
generation is indexed and rendered through normalized tool facts and transcript
evidence.

Agy attribution caveats: copied agy JSONL is detected by record shape, planned
tool calls are not attribution facts, command status is tri-state
(`true`/`false`/unknown), missing agy exit/success is unknown, and runtime
support is best-effort using `created_at` plus executed action records.
Optional agent-mux sidecars can enrich agy model or missing cwd, but core Gaal
does not require agent-mux.

Hermes is useful but should be treated as experimental. It reads a SQLite state
database, not JSONL. `find-salt` does not support Hermes, and Hermes session IDs
often need the full ID to avoid collisions. Environment overrides are
`HERMES_STATE_DB` for a specific state database and `HERMES_HOME` for a home
directory containing `state.db`.

For indexed-session filtering, `--engine claude|codex|gemini|agy|hermes` is
supported on:

```text
gaal ls
gaal who
gaal search
gaal activity
gaal resolve
gaal index backfill
```

`gaal create-handoff --engine` is different: it selects the LLM extraction
engine for handoff generation rather than filtering indexed source sessions.

Do not add source-session `--engine` filters to `inspect`, `recall`,
`transcript`, `find-salt`, or `tag`.

## Index And Tags

Index commands maintain derived state:

```bash
gaal index status
gaal index backfill
gaal index backfill --engine claude
gaal index backfill --with-markdown
gaal index reindex 249aad1e
gaal index recover-orphans --dry-run
```

`backfill` is incremental. It indexes discovered sessions and facts; it does
not generate handoffs.

Tag syntax:

```bash
gaal tag 249aad1e research
gaal tag 249aad1e --remove research
gaal tag ls
```

## Exit Codes

Stable exit codes let agents branch safely:

| Code | Meaning | Typical response |
|------|---------|------------------|
| `0` | success | Parse stdout JSON |
| `1` | no results | Broaden filters or check index/handoff availability |
| `2` | ambiguous ID | Use a longer ID prefix or `gaal ls` |
| `3` | not found | Verify ID or run `gaal index backfill` |
| `10` | no index | Run first-launch index setup |
| `11` | parse error | Fix syntax; inspect the JSON error `example` and `hint` |

## Anti-Patterns

| Do not | Do instead |
|--------|------------|
| Read raw session files for normal work | Use `inspect`, `transcript`, `who`, `search`, or `recall` |
| Treat `activity` as live process monitoring | Use it for historical/indexed activity only |
| Run `recall` and assume no results means no history | Check handoff counts and search raw facts |
| Run handoff batches cold | Start with `create-handoff --batch --dry-run` |
| Add `--engine` to every command | Use it only on commands that support it |
| Loop over `inspect` calls | Use `gaal inspect --ids a,b,c` |
| Pipe `who` directly into another command | Capture output first, then pipe |

## References

Skill-local references:

- `skill/references/verb-reference.md` - verified command and flag reference.
- `skill/references/exit-codes.md` - stable exit code handling.
- `skill/references/troubleshooting.md` - common failure modes and recovery.

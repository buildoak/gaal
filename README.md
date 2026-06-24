# gaal

Gaal makes local AI coding-agent traces searchable, attributable, and handoff-ready.

[![crates.io](https://img.shields.io/crates/v/gaal.svg)](https://crates.io/crates/gaal)
[![CI](https://github.com/buildoak/gaal/actions/workflows/ci.yml/badge.svg)](https://github.com/buildoak/gaal/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS-lightgrey)

I built Gaal because my agents were doing real work and leaving behind almost
no usable memory. Claude Code, Codex, Gemini CLI, Antigravity CLI (`agy`), and
Hermes Agent all write session traces, but each one writes a different shape of
log. After enough
sessions, "I know we solved this last week" becomes an archaeological problem.

Gaal is the small tool I wanted in that moment: index the local traces, normalize
the useful facts, and let a human or a future agent ask practical questions.
Who changed this file? Which session ran that command? What did yesterday's
coordinator actually do? Is there already a handoff for this work?

Not a cloud platform. Not a daemon. Just a local memory layer for agent work.

## Quick Start

Requires Rust 1.80+.

```bash
git clone https://github.com/buildoak/gaal.git
cd gaal
cargo install --path .
```

Index your existing local session traces:

```bash
gaal index backfill
gaal index status
```

Then ask the first useful question:

```bash
gaal ls -H --limit 5
```

On a brand-new machine, `index backfill` can succeed with zero sessions if no
supported agent has written traces yet. Run an agent for a bit, then backfill
again.

By default, Gaal stores its derived database, Tantivy index, rendered
transcripts, config, and generated handoffs under `~/.gaal/`. In CI or sandboxed
agent runs, move that state somewhere disposable:

```bash
export GAAL_HOME=/tmp/gaal-home
gaal index backfill
gaal ls -H --limit 5
```

`GAAL_HOME` does not move the source traces written by Claude Code, Codex,
Gemini CLI, Antigravity CLI, or Hermes Agent. It only changes where Gaal stores
its own derived state.

## Privacy

Core Gaal is local: indexing, fleet views, inspection, transcript rendering,
search, attribution, tags, and recall all operate over files on your machine.

Those local traces can still contain secrets, private prompts, source code,
terminal output, file paths, credentials accidentally printed into logs, and
tool results. Treat `~/.claude/`, `~/.codex/`, `~/.gemini/`, `~/.hermes/`, and
`~/.gaal/` as private working data unless you have audited them.

`gaal create-handoff` is different. It uses the configured LLM/agent backend
and may consume subscription quota, API credits, metered usage, or local
compute. The default supported backend for real handoff generation is
[agent-mux](https://github.com/buildoak/agent-mux). Agent-mux is optional for
core Gaal; it is only needed when you want Gaal to generate handoff artifacts.

Run handoff generation with `--dry-run` first.

## First Useful Commands

```bash
gaal ls -H --limit 10
gaal inspect latest -H
gaal inspect latest --files write
gaal transcript latest
gaal transcript latest --stdout
gaal activity --since 1d
gaal who wrote src/main.rs --since 30d -H
gaal who ran cargo --failed --limit 20 -H
gaal search "migration error" --field all --limit 10 -H
gaal recall "release prep" --limit 3 -H
gaal resolve 3c18caec -H
```

Default output is JSON for normal query commands. Add `-H` or `--human` when
you want a table or card instead of machine-readable output. Two practical
exceptions matter for agents: `gaal salt` prints a raw token string, and CLI
argument-parser errors may be plain text before Gaal's JSON error formatter
runs.

Replace sample session IDs with IDs from your own `gaal ls` output.

## What Gaal Answers

- Which sessions happened recently, across supported engines?
- Which files did a session read, write, or edit?
- Which session wrote a file or ran a command?
- What errors or failed shell commands happened in a time window?
- What did a long session look like as a readable transcript?
- Which past generated handoffs are relevant to this topic?
- Where is the raw source trace for this short session ID?
- Can a Claude Code, Codex, or agy agent identify its own session and leave
  continuity behind?

The core idea is boring in the best way: agent sessions should be queryable
artifacts, not mystery blobs.

## Handoffs

Handoffs are the part that made Gaal feel like more than log search.

A handoff is a compact markdown artifact generated from a session trace:
headline, projects, keywords, useful summary, and enough continuity for the next
agent to pick up the thread. Once generated, handoffs are indexed and retrieved
by `gaal recall`.

They are optional. You can use `ls`, `inspect`, `transcript`, `activity`, `who`,
`search`, `resolve`, `salt`, `find-salt`, `index`, and `tag` without installing
agent-mux or calling any LLM backend.

For one session, using an ID from `gaal ls`:

```bash
gaal create-handoff 3c18caec --dry-run
gaal create-handoff 3c18caec
gaal recall --id 3c18caec --format handoff -H
```

For a running agent that needs to identify itself:

```bash
gaal salt
# GAAL_SALT_716a02ca9642c721
```

Run the second command in a later tool call, after the salt token has been
written into the session log:

```bash
gaal find-salt GAAL_SALT_716a02ca9642c721 -H
```

`find-salt` scans Claude Code and Codex JSONL plus Antigravity brain
transcripts. For agy, it matches salt output in executed action records and
ignores user prompt echoes.

If that returns a JSONL path and you want a continuity artifact:

```bash
gaal create-handoff --jsonl /path/to/session.jsonl --dry-run
gaal create-handoff --jsonl /path/to/session.jsonl
```

The split matters. The salt has to exist in the trace before Gaal can find it.
Tiny thing. Surprisingly useful.

Provider note: `gaal create-handoff --help` may show provider selectors. Real
execution is currently supported through `agent-mux`; dry-run output reports
whether the chosen provider is supported before anything is generated.

## Examples

These examples are shaped from live output and sanitized for public docs.

Fleet view:

```bash
gaal ls -H --limit 3
```

```text
ID        Type        Task            Engine  Started      Duration  Tokens    Peak  Tools  Model    CWD
--------  ----------  --------------  ------  -----------  --------  --------  ----  -----  -------  -------------
3c18caec  [worker]    Rewrite REA...  codex   today 15:56  55s       25K / 2K  43K   16     gpt-5.5  agent-project
4a17843a  [explorer]  Audit CLI h...  codex   today 15:50  3m 10s    99K / 8K  114K  47     gpt-5.5  agent-project
bd914364  [worker]    Fix handof...   codex   today 15:50  2m 26s    51K / 6K  54K   31     gpt-5.5  agent-project
```

Resolve a session:

```bash
gaal resolve 3c18caec -H
```

```text
Session:    3c18caec (gpt-5.5, worker)
JSONL:      ~/.codex/sessions/2026/06/22/rollout-...-3c18caec.jsonl
Transcript: ~/.gaal/data/codex/sessions/2026/06/22/3c18caec.md [ok]
Handoff:    ~/.gaal/data/codex/handoffs/2026/06/22/3c18caec.md [not generated]
```

Preview handoff generation:

```bash
gaal create-handoff latest --dry-run
```

```json
[
  {
    "session_id": "3c18caec",
    "status": "dry_run",
    "provider": "agent-mux",
    "provider_supported": true,
    "estimated_llm_calls": 1,
    "side_effects": {
      "spawn_provider_worker": false,
      "spend_tokens": false,
      "write_handoff_markdown": false,
      "upsert_db_rows": false,
      "index_jsonl": false
    }
  }
]
```

On the author's local machine during release prep, `gaal index status` reported
about 19k sessions, 700k+ indexed facts, and 1k+ handoffs. That is real-world
scale, not a benchmark promise and not a demo dataset requirement.

## Architecture

```text
local agent traces
  -> discovery
  -> parser per engine
  -> normalized sessions + facts
  -> SQLite + Tantivy
  -> CLI commands
  -> JSON or human output
```

Gaal currently indexes:

- Claude Code JSONL traces
- Codex JSONL traces
- Gemini CLI JSON session files
- Antigravity CLI (`agy`) JSONL traces, experimentally
- Hermes Agent SQLite session stores, experimentally

Agy support is native, independent, and currently experimental. It discovers sessions under
`~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`,
falling back to `transcript.jsonl` when the full transcript is absent. Gaal
stores the 8-character agy ID prefix to match Gemini-style short IDs; the full
brain UUID remains recoverable from the source path.

Copied agy JSONL can still be detected by content shape even outside the native
brain directory. Optional agent-mux sidecar metadata can enrich agy rows with a
model or missing cwd when a matching sidecar exists; core Gaal does not require
agent-mux for agy indexing, search, activity, attribution, transcript rendering,
or resolve.

The experimental label means the supported contract is current Antigravity brain
transcript JSONL plus fixture-backed copied JSONL. Token/cost parity and
SQLite/blob sidecars are not claimed yet.

Hermes support is useful but cautious. It has been tested against one real
installation/version plus sanitized fixtures; broader Hermes layouts should be
treated as compatibility work until there are fixtures for them.

The database stores structured session metadata and extracted facts. Tantivy
handles BM25 full-text search. Rendered transcripts and generated handoffs live
as markdown files under `~/.gaal/data/{engine}/...`.

Session classification is deterministic:

| Type | Meaning |
| --- | --- |
| `standalone` | Normal session, no known child agent sessions |
| `coordinator` | Parent session that spawned child agent sessions |
| `subagent` | Child session spawned by a parent |

Attribution follows those relationships, so `gaal who wrote path/to/file` can
show both the child session and the parent chain when the source traces expose
that link.

## What It Does Not Do

- It does not upload your traces for core indexing or search.
- It does not sanitize secrets out of source logs for you.
- It does not monitor live processes or act as a daemon.
- It does not tail sessions in real time; use normal shell tools for that.
- It does not make every session worth preserving. Handoffs are for sessions
  that matter.
- It does not provide agy token/cost parity or parse Antigravity SQLite/blob
  sidecars.
- It does not promise broad Hermes compatibility yet.

If a monitoring feature looks conspicuously absent, there is a decent chance it
was removed because local traces were the more reliable source of truth.

## Command Reference

Run `gaal --help` and `gaal <command> --help` for the full current contract.

### Query

| Command | Purpose |
| --- | --- |
| `gaal ls` | Fleet view across sessions. Useful filters include `--engine`, `--session-type`, `--subagent-type`, `--since`, `--before`, `--cwd`, `--tag`, `--sort`, `--limit`, `--aggregate`, `--all`, and `--skip-subagents`. |
| `gaal inspect <id>` | Session details. Focus with `--files`, `--errors`, `--commands`, `--git`, `--tokens`, `--trace`, `--source`, `--ids`, or `--tag`. |
| `gaal transcript <id>` | Render or locate session transcript markdown. Use `--stdout` to print the markdown. |
| `gaal activity` | Render source-backed transcript slices across a time window. Filters include `--since`, `--before`, `--engine`, `--cwd`, `--session`, and `--skip-subagents`. |
| `gaal who <verb> <target>` | Attribution query. Verbs: `read`, `wrote`, `ran`, `touched`, `changed`, `deleted`. |
| `gaal search <query>` | Full-text search over indexed facts. Filter with `--since`, `--cwd`, `--engine`, `--field`, and `--limit`. |
| `gaal recall <query>` | Search generated handoffs. Use `--id <id>` for direct lookup, `--format` for output shape, and `--substance` to filter low-signal handoffs. |
| `gaal resolve <id>` | Resolve a short session ID to source trace, transcript, handoff path, engine, type, and related metadata. |

### Continuity

| Command | Purpose |
| --- | --- |
| `gaal salt` | Emit a unique token for self-identification. |
| `gaal find-salt <token>` | Find the Claude Code, Codex, or agy JSONL file containing that token in tool/action output and return enriched session metadata when indexed. |
| `gaal create-handoff <id>` | Generate a handoff markdown artifact through the configured backend. Use `--dry-run` first. Agent-mux is the default supported backend for real execution. |
| `gaal create-handoff --jsonl <path>` | Generate from an explicit JSONL file path. Useful after `find-salt`. |
| `gaal create-handoff --batch --since 1d --min-turns 3 --dry-run` | Preview batch handoff candidates before generating anything. |

### Maintain

| Command | Purpose |
| --- | --- |
| `gaal index backfill` | Incrementally index discovered sessions. Supports `--engine`, `--since`, `--force`, `--with-markdown`, and `--output-dir`. |
| `gaal index status` | Show index health and counts. |
| `gaal index reindex <id>` | Force re-index of one session. |
| `gaal index prune --before <date>` | Remove old facts before a date. |
| `gaal index import-eywa` | Import legacy handoff index data. |
| `gaal index recover-orphans` | Recover orphaned subagent files whose parent trace was deleted. |
| `gaal tag <id> <tag>` | Add a local tag to a session. |
| `gaal tag <id> --remove <tag>` | Remove a local tag from a session. |
| `gaal tag ls` | List known tags. |

## Docs

Full docs live in [`docs/`](./docs/):

- [Getting started](./docs/getting-started.md)
- [Architecture](./docs/architecture.md)
- [Command reference](./docs/commands/)
- [Agent guide](./docs/agent-guide.md)
- [Formats and exit codes](./docs/formats.md)

## Built With

- [Rust](https://www.rust-lang.org/) edition 2021
- [rusqlite](https://github.com/rusqlite/rusqlite)
- [tantivy](https://github.com/quickwit-oss/tantivy)
- [clap](https://github.com/clap-rs/clap)
- [serde](https://serde.rs/) and [serde_json](https://github.com/serde-rs/json)

No async runtime. Synchronous by design.

## License

MIT. See [LICENSE](LICENSE).

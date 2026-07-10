# gaal

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS-lightgrey)

**Turn harness logs into transparent and traceable knowledge layer**

Gaal is `strace` for AI coding agents: a local CLI for traceability,
observability, and continuity across agent sessions.

It indexes local traces from Codex, Claude Code, Grok Build, Antigravity CLI
(`agy`), Hermes Agent, and <span style="color:#6a737d">Gemini CLI</span>, then gives
agents one precise way to list, search, inspect, attribute, resolve, render,
recall, and optionally hand off sessions.

Not a cloud platform. Not a daemon. Raw traces stay the evidence; Gaal builds
the smaller token-efficient markdown views and navigation tools around them.

Agent-first is literal here: JSON by default, source-backed, and built for the
next run that has to inherit the mess.

## Install

```bash
brew install buildoak/tap/gaal
# or
cargo install gaal-cli

gaal onboard --dry-run
gaal index backfill
gaal ls -H --limit 5
```

The crates.io package is `gaal-cli` because `gaal` was already occupied there;
it still installs the `gaal` binary.

If you are giving this to a coding agent, use this prompt:

```text
Install or update Gaal, then load the latest Gaal skill from
https://github.com/buildoak/gaal/tree/master/skill.
Run `gaal onboard --dry-run`, follow its first-launch commands, and do not
enable scheduled indexing, install agent-mux, or generate handoffs unless I
explicitly approve that step.
```

## Why This Exists

It started with a naive dream: interactions with AI should compound. Not just
produce output, not just create another pile of logs somewhere, but actually
leave behind usable evidence. Longer story in
[Gaal: strace for agents](https://www.nickoak.com/posts/gaal-dornik/).

Building yet another "memory" felt like faux pas. The logs already existed. The
problem was that they were scattered across engines, enormous in raw form, and
practically hostile to the next agent trying to understand what happened.

Gaal keeps the territory intact and builds maps over it: indexed facts for
finding the right session, token-efficient markdown views for reading it, and
optional handoffs for the cases where a compact continuity note is actually
useful.

## General Intuition

**The idea is simple: render token-efficient markdown views from raw logs, not
one giant slop file; then build an index that lets agents find the right view
precisely.**

**The markdown is the substance an agent reads on demand. The index is how it
gets there without loading the universe. Want to know which session produced a
file? Easy. Want to recover the brilliant prompt where you described the whole
GTM concept three weeks ago? Two or three tool calls, not archaeology.**

The shape is layered:

```text
raw evidence / raw data
  JSONL, SQLite, and harness-owned artifacts left intact

searchable atoms
  normalized facts: sessions, prompts, files, commands, errors, tags,
  artifacts, model, cwd

deterministic markdown views
  almost lossless transcripts and source-backed activity slices

optional compression
  customizable handoffs, generated only when continuity needs it

discovery / drill-down
  start with precise search, then open the smallest useful view
```

The index is the discovery layer. It stores boring, useful facts so an agent can
ask narrow questions before spending tokens on a full session: which sessions
touched this file, which commands failed, which model ran, which project was
active, which artifacts were produced.

The markdown views are the reading layer. A deterministic markdown view is
rendered from source traces plus DB facts. No LLM decides what mattered. Same
source data, same renderer version, same view structure. In Gaal, these views
are typically per session; activity slices are the source-backed exception for
time windows and checkouts.

For transcripts, that means frontmatter, `# Session:`, `## Executive Summary`,
`## Conversation`, and optional open threads or subagent activity. The point is
not to compress the session into vibes. The point is to make the session
readable without breaking its link to the underlying trace.

<details>
<summary>Example: transcript shape</summary>

```markdown
---
session_id: 3c18caec
date: 2026-06-22
duration: 27m
model: gpt-5.5
turns: 12
render_version: 2
---

# Session: Fix transcript renderer

## Executive Summary

### Files Touched (Main Session)

**Read (2):**
- `src/render/session_md.rs`
- `README.md`

### Commands Executed

- `cargo test render::session_md`

## Conversation

### [10:15] User
Please make the transcript view easier to understand.

### [10:17] Codex
I checked the renderer and will keep the example source-backed.
```

</details>

Handoffs are different. They are the optional compression layer: generated
markdown artifacts with searchable metadata such as headline, projects,
keywords, and substance. Useful when the next agent needs continuity quickly.
Not replacement evidence.

<details>
<summary>Example: handoff shape</summary>

```markdown
---
session_id: 3c18caec
date: 2026-06-22
duration: 27
model: gpt-5.5
engine: codex
headline: Fix transcript renderer
projects: [gaal]
keywords: [transcript, markdown, renderer]
substance: 2
---

## Headline
Fix transcript renderer

## What Happened
- Checked the session markdown renderer.
- Reworked the README explanation around raw evidence, facts, and views.

## Key Decisions
- Keep raw traces as evidence.
- Treat handoffs as continuity notes, not ground truth.

## Open Threads
- Decide whether the README also needs a visual diagram.

## Key Files
- `README.md` - public explanation of the Gaal model
```

</details>

The normal path is: broad query -> exact session -> faithful view -> raw
evidence if needed.

## What Agents Can Do With Gaal

Gaal is not a nicer log browser. It is a way for agents to operate over their
own history without pretending memory is magic.

The command snippets below are agent-side moves: an agent asks for evidence,
does something like `gaal search`, `gaal inspect`, or `gaal transcript`, then
uses the result to continue the job.

1. Re-enter old work with evidence

   A future agent can find the relevant prior session, inspect what happened,
   and load the exact transcript only when needed.

   Useful when you have done some properly strange pipeline once and want to
   tell the next agent: "remember that thing where we moved data from here to
   there and somehow got it done? Use Gaal, recall how it worked, then do it
   again."

   ```bash
   gaal recall "auth migration"  # search generated continuity handoffs
   gaal search "auth migration"  # search indexed facts from raw traces
   gaal inspect <id>             # show session metadata, files, commands, tokens
   gaal transcript <id>          # render or locate the markdown transcript
   ```

2. Attribute changes across agents

   When a file changed and nobody remembers which model touched it, an agent
   can ask the traces instead of guessing.

   ```bash
   gaal who wrote README.md --since 30d
   gaal who ran cargo --failed --limit 20
   gaal inspect <id> --files write
   ```

3. Audit fanouts and tune weaker models

   A practical loop: launch 20 `gpt-5.4-mini` xhigh workers, then use Gaal to
   inspect the failures as a corpus. Which commands failed? Which files did
   agents miss? Which prompt shape collapsed? Fix the tool contract, tighten the
   prompt, run again. This is the 100x workflow for making weaker, cheaper
   models useful.

   ```bash
   gaal ls --since 1d --session-type subagent --sort duration -H
   gaal who ran cargo --failed --since 1d --limit 20 -H
   gaal inspect <failed_id> --errors -H
   gaal transcript <failed_id> --stdout
   ```

4. Build source-backed checkouts

   Agents can turn a day, project, or investigation window into a compact
   activity bundle backed by real trace facts. Useful for day checkouts,
   retros, and "what actually happened here?" moments.

   ```bash
   gaal activity --since 1d
   gaal activity --since 2026-06-01
   ```

5. Identify themselves and leave continuity

   A running agent can discover its own session ID, then optionally create a
   handoff for the next agent. No mystical self-awareness. Just a salt token in
   the trace.

   ```bash
   gaal salt
   gaal find-salt <token>
   gaal create-handoff <id> --dry-run
   ```

Default output is JSON for normal query commands. Add `-H` or `--human` when
you want a table or card. `gaal salt` intentionally prints a raw token string,
and CLI argument-parser errors may be plain text before Gaal's JSON error
formatter runs. `gaal transcript` and `gaal activity` return path metadata by
default; add `--stdout` when you want markdown printed to stdout.

## Supported Engines

All supported engines are normalized into the same session, fact, transcript,
tag, and handoff model. "Supported" means covered by the current parser and
discovery surface, not a promise that upstream log formats will never change.
The version column records the last harness version or state schema I have
validated locally; it is not a declared minimum.

| Engine | Source | Last known working harness | Status |
| --- | --- | --- | --- |
| Codex | `~/.codex/sessions/.../rollout-*.jsonl` | `codex-cli 0.141.0` | Supported |
| Claude Code | `~/.claude/projects/.../*.jsonl` | `2.1.126 (Claude Code)` | Supported |
| Antigravity CLI (`agy`) | `~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`, falling back to `transcript.jsonl` | `agy 1.0.11` | Supported |
| Hermes Agent | `~/.hermes/state.db` or `HERMES_STATE_DB` / `HERMES_HOME` overrides | `state.db` schema `6`; binary version not exposed | Experimental |
| Grok Build | `${GROK_HOME:-~/.grok}/sessions/<encoded-cwd>/<uuid>/` | `grok 0.2.93` (`grok-4.5`) | Native, experimental |
| <span style="color:#6a737d">Gemini CLI</span> | `~/.gemini/tmp/*/chats/session-*.json` | `0.36.0` | Supported, legacy-ish |

Agy and Grok support are native: Gaal discovers and parses their local source
artifacts directly. Hermes support is useful but newer; it has been tested
against one real installation shape plus sanitized fixtures.

## Quick Start

Gaal is skill-first. The intended first user is an agent, not a human memorizing
flags. Give your coding agent the repo link and ask it to install Gaal end to
end.

Package-manager install:

```bash
brew install buildoak/tap/gaal
cargo install gaal-cli
```

The crates.io package is `gaal-cli` because `gaal` was already occupied there;
it still installs the `gaal` binary.

Existing installs can be refreshed with:

```bash
brew upgrade buildoak/tap/gaal
cargo install gaal-cli --force
```

After the binary is on `PATH`, the first command is:

```bash
gaal onboard
```

`gaal onboard --dry-run` explains the same flow without implying any local
changes. It tells the installing agent to take the latest skill and first-run
reference from the repo:

```text
https://github.com/buildoak/gaal/tree/master/skill
https://github.com/buildoak/gaal/blob/master/skill/SKILL.md
https://github.com/buildoak/gaal/blob/master/skill/references/first-run.md
```

For source install from the checkout:

```bash
git clone https://github.com/buildoak/gaal.git
cd gaal
./install.sh --no-schedule
```

The installer explains what it is about to do before it changes anything:
install from source, verify the binary, build the first local index, and show
the zero-session state if this machine has no supported traces yet. It does not
silently install background jobs, install agent-mux, generate handoffs, or call
LLM backends.

For manual install without the helper:

```bash
cargo install --path . --force
```

Index your existing local session traces if you skipped the helper:

```bash
gaal index backfill
gaal index status
```

Verify that Gaal can see local sessions:

```bash
gaal ls -H --limit 5
```

On a brand-new machine, `gaal index backfill` can succeed with zero sessions if
no supported agent has written traces yet. Run an agent for a bit, then backfill
again.

Recommended, explicit scheduling:

```bash
./install.sh --schedule
./install.sh print-plist
./install.sh status
```

The scheduled job runs one unfiltered `gaal index backfill`, which includes
Grok and every other supported engine by default. It does not create handoffs,
run recall, call agent-mux, or use an LLM backend. Do not add a second
Grok-only scheduled job.

Optional handoff backend setup:

```bash
./install.sh handoff-setup
./install.sh handoff-setup --install-agent-mux
```

Core Gaal does not need agent-mux. Handoff generation does: agent-mux is the
preferred real-execution backend once configured. Setup checks agent-mux and
runs only `gaal create-handoff latest --dry-run`; real handoff generation
always remains a separate user-approved command.

For the full first-run contract, including storage paths, scheduled indexing,
handoff safety, and first useful prompts, read
[`skill/references/first-run.md`](skill/references/first-run.md).

By default, Gaal stores its derived database, Tantivy index, rendered
transcripts, config, and generated handoffs under `~/.gaal/`. In CI or
sandboxed agent runs, move that derived state somewhere disposable:

```bash
export GAAL_HOME=/tmp/gaal-home
gaal index backfill
gaal ls -H --limit 5
```

`GAAL_HOME` does not move the source traces written by the agent tools. It only
changes where Gaal stores its own derived state.

## Privacy And Locality

Core Gaal is local: indexing, fleet views, inspection, transcript rendering,
search, attribution, tags, and recall over existing handoffs operate over files
on your machine.

Those files can still contain private prompts, source code, file paths, command
output, tool results, and credentials accidentally printed into terminals.
Treat `~/.claude/`, `~/.codex/`, `~/.gemini/`, `~/.hermes/`, `~/.grok/`, and
`~/.gaal/` as private working data unless you have audited them. Grok source
files are never rewritten: private/thought surfaces are excluded from derived
facts, visible Grok text receives best-effort pattern redaction, and projected
tool output is size-bounded. Those safeguards reduce exposure; they are not a
guarantee that arbitrary trace content is safe to publish.

`gaal create-handoff` is different from the read path. It uses the configured
LLM/agent backend, defaults to
[agent-mux](https://github.com/buildoak/agent-mux) for real execution, and may
transmit transcript content or consume subscription quota, API credits, metered
usage, or local compute. Once a handoff exists, `gaal recall` searches that
local handoff index; the generation step is the part that may leave the
machine. Run `--dry-run` first, especially for batch work.

## Handoffs

Handoffs are optional compression for continuity.

A handoff is a generated markdown artifact for one session: headline, projects,
keywords, substance score, useful summary, and enough context for the next agent
to pick up the thread. Once generated, handoffs are indexed and retrieved by
`gaal recall`.

You do not need handoffs for the core workflow. `ls`, `inspect`, `transcript`,
`activity`, `who`, `search`, `resolve`, `salt`, `find-salt`, `index`, and `tag`
work without installing [agent-mux](https://github.com/buildoak/agent-mux) or
calling any LLM backend.

For one known session:

```bash
gaal create-handoff <id> --dry-run
gaal create-handoff <id>  # only after review and a continuity need
gaal recall --id <id> --format handoff -H
```

For a running agent that needs to identify itself, use two separate tool calls.
The salt has to appear in the session log before Gaal can find it:

```bash
gaal salt
```

```bash
gaal find-salt GAAL_SALT_<hex> -H
```

If that returns a source path and a handoff is appropriate:

```bash
gaal create-handoff --jsonl /path/to/source --dry-run
gaal create-handoff --jsonl /path/to/source  # only after review and a continuity need
```

`find-salt` scans Claude Code, Codex, agy logs, and Grok visible session source
artifacts for executed tool/action output. For Grok, the legacy `jsonl_path`
field contains the session directory because a native Grok session is a
multi-file artifact. It does not identify Gemini JSON sessions or Hermes SQLite
sessions.

Provider caveat: real handoff execution is currently supported through
[agent-mux](https://github.com/buildoak/agent-mux). The CLI may expose other
provider selectors for planning or dry-run compatibility; trust
`provider_supported` in `--dry-run` output before running any non-dry-run
generation.

## Examples

These examples are shaped from live output and sanitized for public docs.

Fleet view:

```bash
gaal ls -H --limit 3
```

```text
ID        Type        Task            Engine  Started      Duration  Tokens    Peak  Tools  Model    CWD
--------  ----------  --------------  ------  -----------  --------  --------  ----  -----  -------  -------------
3c18caec  [worker]    Rewrite REA...  codex   today 15:56  55s       25K / 2K  43K   16     gpt-5.4  agent-project
4a17843a  [explorer]  Audit CLI h...  codex   today 15:50  3m 10s    99K / 8K  114K  47     gpt-5.4  agent-project
bd914364  [worker]    Fix handof...   codex   today 15:50  2m 26s    51K / 6K  54K   31     gpt-5.4  agent-project
```

Resolve a session:

```bash
gaal resolve 3c18caec -H
```

```text
Session:    3c18caec (gpt-5.4, worker)
Source:     ~/.codex/sessions/2026/06/22/rollout-...-3c18caec.jsonl
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

## Architecture

```text
engine source artifacts on disk
  -> discovery
  -> parser per engine
  -> normalized sessions + facts
  -> SQLite + Tantivy
  -> CLI commands
  -> JSON, human tables, or markdown renders
```

SQLite stores structured session metadata, normalized facts, tags, handoff
metadata, and Hermes aliases. Tantivy handles BM25 full-text search over facts.
Rendered transcripts and generated handoffs live as markdown files under
`~/.gaal/data/{engine}/...`.

Session classification is deterministic:

| Type | Meaning |
| --- | --- |
| `standalone` | Normal session, no known child agent sessions |
| `coordinator` | Parent session that spawned child agent sessions |
| `subagent` | Child session linked to a parent |

Attribution follows those relationships, so `gaal who wrote path/to/file` can
show both the child session and the parent chain when the source traces expose
that link.

## What It Does Not Do

- It does not upload your traces for core indexing, fleet views, inspection,
  transcript rendering, search, attribution, tags, or recall over existing
  handoffs.
- It does not sanitize secrets out of source logs for you.
- It does not monitor live processes or act as a daemon.
- It does not tail sessions in real time; `activity` is historical and
  index-backed.
- It does not make every session worth preserving. Handoffs are for sessions
  that matter.
- It does not provide agy token/cost parity or parse Antigravity SQLite/blob
  sidecars.
- It does not promise broad Hermes compatibility yet.

## Command Reference

Run `gaal --help` and `gaal <command> --help` for the full current contract.

### Query

| Command | Purpose |
| --- | --- |
| `gaal ls` | Fleet view across indexed sessions. |
| `gaal inspect <id>` | Structured detail for one session, or batch mode with `--ids`. |
| `gaal transcript <id>` | Render or locate markdown transcript output. Use `--stdout` to print the markdown. |
| `gaal activity` | Create source-backed historical activity slices across sessions. Default output is path metadata; use `--stdout` to print markdown. |
| `gaal who <verb> <target>` | Attribution query for `read`, `wrote`, `ran`, `touched`, `changed`, or `deleted`. |
| `gaal search <query>` | Full-text search over indexed facts. |
| `gaal recall <query>` | Search generated local handoffs. Use `--id <id>` for direct handoff lookup. |
| `gaal resolve <reference>` | Resolve an indexed full ID, unique prefix, registered alias, or `latest` to source and artifact paths. |

### Continuity

| Command | Purpose |
| --- | --- |
| `gaal salt` | Emit a unique token for self-identification. |
| `gaal find-salt <token>` | Find the Claude Code, Codex, agy, or Grok source artifact containing that token in executed tool/action output, with indexed session context when available. |
| `gaal create-handoff <id>` | Generate a handoff markdown artifact through the configured backend. Use `--dry-run` first; [agent-mux](https://github.com/buildoak/agent-mux) is the supported real-execution provider today. |
| `gaal create-handoff --jsonl <path>` | Generate from an explicit source artifact path (the flag name is legacy and also accepts a Grok session directory); preview with `--dry-run` first. |
| `gaal create-handoff --batch --since 1d --min-turns 3 --dry-run` | Preview batch handoff candidates before generating anything. |

### Maintain

| Command | Purpose |
| --- | --- |
| `gaal index backfill` | Incrementally index all discovered supported engines, including Grok. Use `--engine` only to restrict a run; `--since`, `--force`, and optional transcript markdown output are also supported. |
| `gaal index status` | Show index health and counts. |
| `gaal index reindex <id>` | Force re-index of one session. |
| `gaal index prune --before <date>` | Remove old indexed facts before a date. |
| `gaal index recover-orphans --dry-run` | Preview recovery of orphaned subagent files before writing derived rows. |
| `gaal tag <id> <tag>` | Add a local tag to a session. |
| `gaal tag <id> --remove <tag>` | Remove a local tag from a session. |
| `gaal tag ls` | List known tags. |

## Session IDs

Session references are lookup handles, and compact aliases are engine-specific. Codex uses
the last 8 UUID hex characters; Claude Code, Gemini, and Agy usually use the
first 8 native ID characters; Hermes keeps the full native ID and adds a
registered alias. Grok keeps the full UUID as canonical/artifact ID and adds a
Codex-style last-8 dash-stripped lookup alias when unique. When unsure, run
`gaal resolve <reference> -H`. Full IDs and unique prefixes remain valid.

## Grok

Native Grok support is included in an unfiltered backfill. Grok Build remains a
fast-moving multi-file trace format, so `--engine grok` is useful for a scoped
diagnostic or repair run, not required for normal indexing:

```bash
gaal index backfill
gaal index backfill --engine grok  # optional Grok-only run
gaal inspect <full-uuid-or-last8> --source
gaal transcript <full-uuid-or-last8>
```

Gaal reads `${GROK_HOME:-~/.grok}/sessions/<percent-encoded-cwd>/<full-uuid>/`.
`updates.jsonl` is the primary visible source; `chat_history.jsonl` is fallback.
Thoughts, prompt context, system prompts, summaries/titles, rewind files, and
unknown Grok files are excluded or reported as private/unknown rather than
indexed as visible facts. Pattern-based secret redaction and tool-output size
limits apply to derived Grok text only; both are defense-in-depth, not a
complete sanitization guarantee. Raw Grok files on disk are not modified.

## Docs

Full docs live in [`docs/`](./docs/):

- [Getting started](./docs/getting-started.md)
- [Philosophy](./docs/philosophy.md)
- [Architecture](./docs/architecture.md)
- [Command reference](./docs/commands/README.md)
- [Agent guide](./docs/agent-guide.md)
- [Formats and exit codes](./docs/formats.md)
- [Hermes engine notes](./docs/hermes-engine-plan.md)

## Built With

- [Rust](https://www.rust-lang.org/) edition 2021
- [rusqlite](https://github.com/rusqlite/rusqlite)
- [tantivy](https://github.com/quickwit-oss/tantivy)
- [clap](https://github.com/clap-rs/clap)
- [serde](https://serde.rs/) and [serde_json](https://github.com/serde-rs/json)

No async runtime. Synchronous by design.

## License

MIT. See [LICENSE](LICENSE).

# First-Run Reference

Use this when Gaal is new on a machine, in a checkout, or in an agent's working
environment. The goal is not just to put a binary on `PATH`. The goal is to
teach the machine and the next agent where session evidence lives, how Gaal
indexes it, and how to verify that the map is usable.

## What Gaal Is

Gaal is `strace` for AI coding agents: a local CLI for traceability,
observability, and continuity across agent sessions.

Agent harnesses already leave evidence on disk. Raw traces are the territory.
Gaal builds maps over them:

```text
raw agent traces
  -> indexed facts for discovery
  -> deterministic markdown views for reading
  -> optional generated handoffs for continuity
  -> scheduled indexing so the map stays fresh
```

Gaal is not cloud memory, not a daemon, and not a live process monitor. The
database is not the memory. It is the routing layer that helps agents find the
right source-backed view before spending context on full transcripts.

## Agent-First Philosophy

The primary user is the next agent inheriting work. Humans should understand
what is installed and why; agents need exact commands, paths, caveats, and
verification gates.

Default output is JSON for agents. Add `-H` or `--human` for tables and cards.
When a command fails after CLI parsing, Gaal-shaped errors should tell the agent
what happened, a valid example, and the next useful command.

For the broader model, see [Gaal philosophy](../../docs/philosophy.md) and the
[agent guide](../../docs/agent-guide.md).

## What Gets Indexed

Gaal indexes local traces from supported CLI-style harnesses:

| Engine | Source boundary |
| --- | --- |
| Codex CLI | `~/.codex/sessions/.../rollout-*.jsonl` |
| Claude Code | `~/.claude/projects/.../*.jsonl` |
| Antigravity CLI (`agy`) | `~/.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`, falling back to `transcript.jsonl` |
| Gemini CLI | `~/.gemini/tmp/*/chats/session-*.json` |
| Hermes Agent | `~/.hermes/state.db`, or `HERMES_STATE_DB` / `HERMES_HOME` overrides |

"Supported" means Gaal has discovery and parser coverage for these local source
shapes. Codex, Claude, Gemini, or other web and desktop UI apps are not
supported unless they write one of these local trace formats.

Indexed facts include sessions, prompts, replies, commands, file reads/writes,
errors, tags, metadata, cwd, model where available, and parent-child session
relationships where source traces expose them. Raw traces remain the authority.

## Where Gaal Stores Derived State

By default Gaal writes derived state under `~/.gaal/`:

```text
~/.gaal/
  index.db
  tantivy/
  config.toml
  prompts/handoff.md
  data/{engine}/sessions/YYYY/MM/DD/<id>.md
  data/{engine}/handoffs/YYYY/MM/DD/<id>.md
```

`index.db` is the SQLite store for session metadata, facts, tags, and handoff
metadata. `tantivy/` is the full-text index used by `gaal search`.
`data/{engine}/sessions/...` holds deterministic transcript markdown.
`data/{engine}/handoffs/...` holds generated continuity handoffs.

Set `GAAL_HOME` to move Gaal's derived state for sandboxes or CI:

```bash
export GAAL_HOME=/tmp/gaal-home
```

`GAAL_HOME` does not move source traces owned by Codex, Claude Code, Gemini,
Antigravity, or Hermes.

## Onboarding Command

After any package-manager install, start with:

```bash
gaal onboard
```

For agentic installation, `gaal onboard --dry-run` is the smallest safe
contract. It does not write files, index sessions, schedule jobs, install
agent-mux, or generate handoffs. It tells the installing agent to use the latest
skill and references from:

```text
https://github.com/buildoak/gaal/tree/master/skill
https://github.com/buildoak/gaal/blob/master/skill/SKILL.md
https://github.com/buildoak/gaal/blob/master/skill/references/first-run.md
```

That keeps the binary install simple while still giving agents the current
operational contract before they touch local traces.

## Source Install

Source install is canonical for the pre-launch path. Do not claim a crates.io
install path until release packaging has actually been verified. A future
crates.io path can be added here later.

```bash
git clone https://github.com/buildoak/gaal.git
cd gaal
./install.sh --no-schedule
gaal --version
```

The installer starts by explaining the plan in plain words before it changes
anything. The default first run is:

1. build and install Gaal from this checkout;
2. verify the installed binary;
3. run the first local index;
4. show sessions if any exist;
5. explain the zero-session state if this machine has no supported traces yet;
6. ask about scheduled indexing only in an interactive terminal.

It does not silently install background jobs, agent-mux, or handoff generation.
Those are separate approval points.

Manual fallback:

```bash
cargo install --path . --force
gaal --version
```

For development, a stable symlink to one checkout can be useful if you want one
local binary path to follow each rebuild:

```bash
cargo build --release
mkdir -p "$HOME/.local/bin"
ln -sf "$PWD/target/release/gaal" "$HOME/.local/bin/gaal"
"$HOME/.local/bin/gaal" --version
```

Before configuring any scheduled job, verify the exact binary path it will run:

```bash
command -v gaal
gaal --version
```

For more build details, see [Getting started](../../docs/getting-started.md).

## First Index

Build the initial local map:

```bash
gaal index backfill
gaal index status
gaal ls -H --limit 5
```

On a machine with no supported traces yet, `gaal ls -H --limit 5` reports the
zero-session state and exits with code `1`. That is a healthy empty map, not a
broken install. The installer treats this as an educational first-run state.

`index backfill` is incremental. It discovers supported source traces, parses
new or changed sessions, writes derived rows under `GAAL_HOME`, and rebuilds
full-text search when new facts were indexed. It does not require an LLM
backend.

If the machine has no supported sessions yet, `index backfill` can succeed with
zero indexed sessions. That is normal. Check:

- a supported agent has run at least once;
- `HOME` points at the profile containing the trace roots;
- `GAAL_HOME` is writable and not confused with source trace storage;
- the harness is one of the supported CLI-style sources above;
- the source roots exist, for example `~/.codex/sessions` or
  `~/.claude/projects`.

For exact index flags and cursor behavior, see
[`gaal index`](../../docs/commands/index-tags.md).

## Scheduled Indexing Doctrine

Scheduled indexing is recommended because agent traces keep arriving and Gaal is
historical by design. It does not watch live processes. A scheduled
`gaal index backfill` keeps the discovery map fresh without asking humans or
agents to remember.

The default scheduled job must run only:

```bash
gaal index backfill
```

No default handoffs. No LLM calls. No transcript-derived content leaving the
machine. Handoff generation is a separate, explicit continuity action.

Do not silently install scheduled indexing from an agent run. Installation must
happen only when the human explicitly asked for scheduling, an installer was
invoked with an explicit `--schedule`-style flag, or an interactive TTY
confirmation was shown and accepted.

## Scheduled Install Contract

The launch-grade scheduled installer should expose inspectable operations:

```bash
./install.sh --schedule
./install.sh --no-schedule
./install.sh --dry-run
./install.sh --print-next-steps

./install.sh status
./install.sh uninstall-schedule
./install.sh print-plist
```

It should verify Rust/Cargo, build or install Gaal, resolve the exact `gaal`
path, run `gaal --version`, explain `~/.gaal` and `GAAL_HOME`, run the first
index checks, and install scheduling only after explicit confirmation or flag.

The scheduled job should use the resolved binary path, preserve stderr in a
visible log, and make `launchctl` state inspectable. It must not assume a
Homebrew or Cargo path unless that is the verified path on this machine.

The implementation surface is:

```bash
./install.sh --dry-run
./install.sh --schedule
./install.sh status
./install.sh print-plist
./install.sh uninstall-schedule

defaults/gaal-cron-install.sh --print-plist
defaults/gaal-cron-install.sh status
```

The helper under [`defaults/`](../../defaults/) generates the LaunchAgent with
the resolved binary path and separate stdout/stderr logs.

## Optional Handoff Backend Setup

Core Gaal works without handoff generation. Indexing, search, attribution,
transcripts, activity, tags, and recall over already-generated handoffs are
local workflows.

Real handoff generation is different: it asks an LLM/agent backend to turn a
session into a compact continuity note. Today the practical backend is
agent-mux. That setup is easy, but it requires direct user approval because it
can install another local tool and later handoff generation can spend quota or
local compute.

To inspect the handoff setup path without changing anything:

```bash
./install.sh handoff-setup --dry-run --no-agent-mux
```

To run the guided handoff setup:

```bash
./install.sh handoff-setup
```

If agent-mux is missing, the script explains the source install and asks before
installing it. To make that approval explicit in a non-interactive command:

```bash
./install.sh handoff-setup --install-agent-mux
```

That may run:

```bash
git clone https://github.com/buildoak/agent-mux.git "$HOME/.local/src/agent-mux"
cd "$HOME/.local/src/agent-mux"
go build -o "$HOME/.local/bin/agent-mux" ./cmd/agent-mux
```

After agent-mux is present, setup checks:

```bash
agent-mux config prompts
agent-mux config engines --json
gaal create-handoff latest --dry-run
```

The last command is still only a preview. It does not generate a handoff, write
handoff markdown, call the provider, or spend tokens. Real generation remains a
separate command the user must choose later:

```bash
gaal create-handoff latest
```

## First Useful Queries

After the first index exists:

```bash
gaal ls -H --limit 5
gaal inspect latest --tokens -H
gaal who wrote README.md --since 30d -H
gaal search "auth migration" --limit 5 -H
gaal transcript latest
gaal activity --since 1d
```

`gaal transcript <id>` returns path metadata by default. Add `--stdout` only
when you want markdown printed into the current context.

Use [`verb-reference.md`](verb-reference.md) for command-specific flags and
[`troubleshooting.md`](troubleshooting.md) when the index, IDs, or handoff
provider path behaves unexpectedly.

## Power-Case Prompts

Give these to an agent after the first index is populated:

```text
Use Gaal to show me my work patterns from the last 7 days. Start broad, inspect
only the smallest useful sessions, and cite the commands you used.
```

```text
Use Gaal to find the session that last touched <path>. Inspect the exact
session, summarize what changed, and point me to the transcript path if useful.
```

```text
Use Gaal to search for prior work on <topic>. Compare `gaal search` over raw
indexed facts with `gaal recall` over generated handoffs, and tell me which
source is stronger.
```

```text
Use Gaal to audit recent failed commands in this repo. Find the most relevant
sessions, inspect errors, and report the smallest reproducible pattern.
```

```text
Use Gaal to build a source-backed activity checkout for today. Start with
`gaal activity --since 1d`, then inspect only the sessions needed to explain
what actually changed.
```

```text
Use Gaal to preview a safe handoff for the most relevant current session. Run
only dry-run commands, explain provider/model/side effects, and stop before real
generation unless I explicitly approve it.
```

```text
Use Gaal to identify your own current session. Run `gaal salt`, then in a
separate tool call run `gaal find-salt <token>`, and report the session ID,
engine, JSONL path, and whether a handoff already exists.
```

```text
Use Gaal to check whether real handoff generation is ready. Verify whether
agent-mux is installed, run a `gaal create-handoff latest --dry-run`, and report
whether `provider_supported` is true before doing anything non-dry-run.
```

## Handoffs Onboarding

Handoffs are optional compression for continuity. They are generated markdown
artifacts for sessions that matter. They are not replacement evidence.

`gaal recall` searches generated local handoffs, not raw traces. If recall
returns nothing useful, use `gaal search`, `gaal who`, `gaal inspect`, and
`gaal transcript` before generating new handoffs.

Preview first:

```bash
gaal create-handoff latest --dry-run
```

Only after reviewing the dry run, confirming the provider, and accepting the
privacy/quota tradeoff:

```bash
gaal create-handoff latest  # intentional real generation
gaal recall --id latest --format brief -H
```

For self-identification from a running Codex, Claude Code, or agy session, use
two separate tool calls so the salt token can flush into the trace:

```bash
gaal salt
```

```bash
gaal find-salt GAAL_SALT_<hex> -H
gaal create-handoff --jsonl /path/to/session.jsonl --dry-run
```

`find-salt` does not identify Gemini JSON sessions or Hermes SQLite sessions.
Batch handoffs are advanced; start with
[`defaults/handoff-batch-advanced.md`](../../defaults/handoff-batch-advanced.md)
and keep filters narrow.

## Agent-Mux Note

Core indexing, search, inspect, transcript, activity, attribution, tags, and
recall over existing handoffs do not require agent-mux.

Real `gaal create-handoff` execution uses the configured LLM/agent backend.
Today the supported real provider is agent-mux. Other provider selectors may
exist for planning or dry-run compatibility; trust `provider_supported` in
`--dry-run` output before any non-dry-run generation.

Readiness check:

```bash
command -v agent-mux
agent-mux --help
agent-mux config prompts
agent-mux config engines --json
gaal create-handoff latest --dry-run
```

If `command -v agent-mux` fails, core Gaal is still usable: keep using
`ls`, `inspect`, `who`, `search`, `transcript`, `activity`, and scheduled
`index backfill`. If `--dry-run` reports `provider_supported: false`, treat
handoff generation as preview-only until the agent-mux installation/profile
roster is repaired.

## Privacy And Safety

Core Gaal operations are local: indexing, listing, inspection, transcript
rendering, search, attribution, tagging, and recall over existing handoffs
operate over files on the machine.

Local does not mean harmless. Agent traces can contain private prompts, source
code, file paths, command output, customer data, secrets accidentally printed to
terminals, and tool results. Treat these as private working data unless audited:

- `~/.claude/`
- `~/.codex/`
- `~/.gemini/`
- `~/.hermes/`
- `~/.gaal/`

`gaal create-handoff` is externally sensitive. It may transmit
transcript-derived content to a configured backend and consume subscription
quota, API credits, metered usage, or local compute. Always use `--dry-run`
before real generation, especially batch generation.

## First Verification Commands

A first-run pass is accepted when these commands prove the binary, index, and
basic query surface work:

```bash
command -v gaal
gaal --version
gaal index backfill
gaal index status
gaal ls -H --limit 5
```

If at least one session is indexed, also verify:

```bash
gaal inspect latest --tokens -H
gaal transcript latest
```

If no sessions are indexed, report that state directly and include the source
roots Gaal is expected to discover. A clean zero-session first run is still a
valid install when the machine has no supported local traces yet.

# CLAUDE.md - Gaal Contributor Notes

Gaal is a local session observability CLI for AI coding agents. It indexes
traces from Codex, Claude Code, Antigravity CLI, Hermes Agent, and Gemini CLI,
then turns those traces into searchable facts, deterministic markdown views,
attribution queries, and optional handoffs.

This file is for agents and contributors working inside the repo. The public
user path starts in `README.md`; the public agent skill starts in
`skill/SKILL.md`.

## Working Rule

Reality first. Agent trace formats are upstream artifacts, not APIs we control.
Before changing a parser, discovery path, transcript renderer, or command
contract, inspect real fixture/source shapes and then update code, docs, skill,
and tests together.

## Architecture Map

```text
src/
  main.rs              CLI entry point and clap surface
  config.rs            GAAL_HOME/config loading
  commands/            ls, inspect, transcript, activity, who, search, recall,
                       create-handoff, salt/find-salt, index, tag, resolve
  discovery/           local trace discovery per engine
  parser/              engine-specific parsers plus normalized fact extraction
  db/                  SQLite schema, migrations, query helpers
  render/              deterministic markdown transcript rendering
  subagent/            parent/child session discovery and attribution
  output/              JSON and human output rendering
tests/
  fixtures/            sanitized engine fixtures
  integration/         Rust integration tests
  ax/                  agent-experience harness scripts and manifests
docs/
  commands/            command reference by group
skill/
  SKILL.md             public agent-facing skill
  references/          install, onboarding, troubleshooting, verb reference
```

## Core Model

Gaal keeps source traces in place. It builds:

- indexed facts for discovery: prompts, commands, file activity, errors,
  metadata, tags, and session relationships
- deterministic markdown transcripts for per-session reading
- activity slices for time-window review
- optional generated handoffs for continuity

The point is not one clever summary. The point is: find the right session,
read the smallest faithful view, and fall back to raw evidence when precision
matters.

## Public CLI Contract

Primary commands:

| Area | Commands |
| --- | --- |
| Discovery | `ls`, `search`, `who`, `resolve` |
| Reading | `inspect`, `transcript`, `activity` |
| Continuity | `recall`, `create-handoff`, `salt`, `find-salt` |
| Maintenance | `index backfill`, `index status`, `index reindex`, `index prune`, `index recover-orphans`, `tag` |

Default output is JSON for agent use. `-H` is for human-readable tables/cards.
Do not introduce human-only output as the only parseable surface for a workflow.

## Feature Kill List

These were removed deliberately. Do not re-add them casually.

| Feature | Why |
| --- | --- |
| `gaal active` | Process monitoring was fragile and misleading. |
| `gaal show` | Folded into `inspect`. |
| Live tail/watch/status flags | Gaal is historical/index-backed, not a daemon. |
| Stuck or loop detection | Too many false signals for too little value. |
| Parent-child linking by PID | Too unreliable across real agent harnesses. |

`gaal activity` is allowed because it renders historical, source-backed slices.
It must not become live process monitoring.

## Change Discipline

For any command or output change:

1. Update the Rust command/parser/output code.
2. Update `README.md` if the change affects top-level use.
3. Update the relevant `docs/commands/*.md`.
4. Update `skill/SKILL.md` or `skill/references/*` if agents need to know.
5. Update tests or fixtures.
6. Run the verification gate below.

For parser/discovery work, prefer sanitized fixtures over private live traces in
the repo. Private run outputs must not be committed.

## Verification Gate

Run at least:

```bash
cargo test
./tests/run-all.sh
```

For public-surface cleanup:

```bash
cargo package --list
rg -n 'TODO|FIXME|private local path|stale legacy migration term' README.md docs skill defaults src tests
```

For CLI contract changes:

```bash
cargo run -- --help
cargo run -- index --help
cargo run -- recall --help
cargo run -- create-handoff --help
```

If a check fails, fix the artifact or document the exact residual risk before
claiming the change is done.

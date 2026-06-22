# Hermes Engine Notes

Status: implemented in 0.3.0; still experimental across unobserved Hermes installations.
Primary gate: Hermes remains Gaal engine #4 without regressing Claude, Codex, or Gemini.

## Objective

Hermes Agent support is implemented as a native fourth engine adapter. This note records the design constraints, fixture policy, and verification gates that keep the adapter honest.

The implementation should preserve Gaal's current architecture:

```text
engine-specific source artifact
  -> engine-specific discovery/parser adapter
  -> canonical SessionEvent stream
  -> shared fact extractor
  -> SQLite/Tantivy
  -> ls / inspect / search / transcript / handoff
```

Hermes-specific behavior belongs behind the Hermes adapter boundary. The shared Gaal core should continue to reason over canonical events, sessions, facts, transcripts, and handoffs.

## Current Evidence

Hermes support was first validated against one private Hermes Agent installation and sanitized local fixtures. Treat that as useful evidence, not a universal compatibility claim.

Observed private-install surfaces:

- Canonical state: `~/.hermes/state.db`
- Main tables: `sessions`, `messages`, `messages_fts`
- Observed scale: 165 sessions in the remote DB at planning time
- Observed sources: `cli`, `telegram`, `cron`
- Legacy/witness files: `~/.hermes/sessions/`
- Compression behavior: Hermes creates continuation sessions linked by `parent_session_id`

Local Gaal surfaces:

- Existing native adapters: Claude, Codex, Gemini
- Normalization target: `SessionEvent` in `src/parser/event.rs`
- Shared extractor: `src/parser/facts.rs`
- Markdown transcript renderer: `src/render/session_md.rs`
- Handoff command: `src/commands/handoff.rs`

## Chosen Design

Use a SQLite-primary native Hermes adapter.

Do not make Gaal depend on live remote access for normal development or tests. Use a private installation as the evidence source only when needed, then build sanitized local fixtures.

```text
private Hermes ~/.hermes/state.db
  -> read-only schema/sample scout
  -> sanitized fixture generator
  -> local fixture DB/JSON
  -> Hermes adapter implementation
  -> local temp GAAL_HOME tests
  -> optional live install and verification on a private test host
```

Rejected for now:

- Generic Gaal trace format first: cleaner long term, but too much abstraction before value.
- Exporting Hermes as fake Claude/Codex logs: fastest but leaks false provenance and couples Hermes to the wrong parser.
- Directly indexing a live private Hermes DB from a development machine: too brittle and privacy-sensitive.

## ID Rule

Store the full Hermes session ID canonically.

Hermes natural IDs are date/time-prefixed and can have short entropy suffixes, for example:

```text
20260422_040102_5a0ab5
cron_44cc6d0e288d_20260422_145759
```

Do not use first-8 prefixes. They collide by date.

Preferred resolver/display strategy:

- canonical DB `sessions.id`: full Hermes session ID
- optional short alias: first 8 hex chars of `sha256(full_hermes_id)`
- enforce uniqueness during indexing
- if an alias collision appears, extend only the colliding aliases

MVI can defer alias persistence if full IDs resolve cleanly. The first implementation may require full Hermes IDs while the resolver story is finished.

## Event Mapping

Hermes adapter emits canonical Gaal events.

Mapping:

- `sessions` row -> `EventKind::Meta`
- session token totals -> `EventKind::Usage`
- `messages.role = user` -> `UserMessage`
- `messages.role = assistant` -> `AssistantMessage`
- assistant `tool_calls` -> `ToolUse`
- `messages.role = tool` -> `ToolResult`
- `messages.role = session_meta` -> metadata only, not a user turn
- compression parent/child -> continuation lineage via `parent_id`, not subagent type

Tool normalization:

- shell/terminal/code execution -> command fact
- file read/search -> file read fact when path is recoverable
- file write/edit/patch -> file write fact when path is recoverable
- web/search/fetch -> searchable tool fact
- unknown tools still count as tools and render in transcript

Privacy rule:

- Do not index `system_prompt` as conversation content by default.
- Do not commit raw private Hermes content.
- Fixtures must preserve schema shape and replace message content with placeholders.

## Implementation Touchpoints

Expected code changes:

- `src/parser/types.rs`: add `Engine::Hermes`
- `src/discovery/hermes.rs`: discover sessions from read-only SQLite
- `src/parser/hermes.rs`: parse one Hermes session from `state.db`
- `src/parser/mod.rs`: route Hermes parse/detect where applicable
- `src/discovery/discover.rs`: add Hermes discovery branch
- `src/commands/index/mod.rs`: add Hermes backfill cursor and full-session reparse path
- `src/db/schema.sql` and `src/db/schema.rs`: allow `engine = 'hermes'`
- `src/main.rs`: add Hermes to CLI engine filters
- `src/render/session_md.rs`: render Hermes transcripts and continuation lineage
- `src/commands/transcript.rs`: ensure Hermes transcript source routing works
- `src/commands/handoff.rs`: allow Hermes transcript/handoff paths and source refs
- `src/commands/resolve.rs` and related ID resolution: handle full Hermes IDs and optional aliases
- docs and tests for new engine filters and workflows

Expected test/fixture changes:

- `tests/fixtures/hermes/`
- parser unit tests for role/tool mappings
- discovery tests against a fixture DB
- integration tests under temp `GAAL_HOME`
- AX layer entries for invalid `--engine hermes` or not-found Hermes IDs if new error paths appear

## Fixture Protocol

Commands against a private Hermes installation must be read-only unless the operator explicitly authorizes mutation.

First pass:

```bash
sqlite3 ~/.hermes/state.db ".schema sessions"
sqlite3 ~/.hermes/state.db ".schema messages"
sqlite3 ~/.hermes/state.db "SELECT source, COUNT(*) FROM sessions GROUP BY source;"
```

Fixture set:

- one CLI session
- one Telegram session
- one cron session
- one compression parent/child pair
- one tool-heavy session

The fixture generator should emit either:

- a tiny sanitized SQLite DB that preserves Hermes table shape, or
- JSON fixtures if a DB fixture is too cumbersome

Prefer sanitized SQLite because it exercises the real adapter boundary.

Sanitization requirements:

- keep IDs, timestamps, roles, sources, parent links, token fields, and tool-call JSON keys
- replace user/assistant/tool text with placeholders
- redact secrets, paths that identify private content, tokens, phone numbers, chat IDs, and credentials
- record the generator command and redaction policy in the fixture README

## Test-Time Compute Plan

Use `test-time-compute` as a controller, not as vague "more thinking".

Objective:
Add Hermes engine support end to end with minimal user involvement and strong evidence.

Gate:

- local fixture indexing works under temp `GAAL_HOME`
- `gaal ls --engine hermes`
- `gaal inspect <hermes-id>`
- `gaal search <fixture term> --engine hermes`
- `gaal transcript <hermes-id>`
- `gaal create-handoff <hermes-id> --dry-run`
- existing Claude/Codex/Gemini tests still pass
- release binary built with `cargo build --release`
- optional live private-host install and smoke test passes after explicit authorization

Budget:

- planning scouts: 3 read-only
- implementation workers: 2 max, disjoint write scopes
- auditor/verifier: 1
- repair pass: 1
- no overlapping write branches without coordinator merge ownership

Verifier:

- sanitized fixture DB inspection
- parser unit tests
- integration commands with temp `GAAL_HOME`
- `cargo test`
- `cargo build --release`
- `./tests/run-all.sh` when feasible
- private-host live smoke test after install

Selection rule:

- choose the smallest implementation that satisfies the gate and keeps Hermes-specific logic isolated
- reject designs that require raw private logs in repo or live SSH for ordinary tests

Stop rule:

- stop when all local gates pass and live private-host verification has either passed or is explicitly blocked

## Worker Routing

Use heavy subagents for the parts that are evidence-heavy or independently verifiable.

Recommended branches:

1. Hermes installation scout
   - read-only SSH
   - output schema, sample keys, fixture candidates, privacy risks
   - no raw message dumps

2. Gaal adapter mapper
   - map current engine hard-coding and exact code touchpoints
   - confirm test surfaces
   - no edits

3. Fixture builder
   - write sanitized fixtures and fixture README
   - owns only `tests/fixtures/hermes/**`

4. Adapter implementer
   - owns `src/parser/hermes.rs`, `src/discovery/hermes.rs`, engine enum and routing
   - does not touch handoff logic unless needed for compile

5. Surface integrator
   - owns CLI filters, schema migration, transcript/handoff path support, docs
   - coordinates with adapter implementer through the coordinator

6. Auditor
   - read-only after implementation
   - checks privacy, regressions, ID resolution, continuation semantics, and test adequacy

The coordinator owns synthesis, merge decisions, final verification, and commits.

## External Install Gate

Installing Gaal on another machine is an external mutation. It requires explicit authorization.

Target live gate after local tests pass:

```bash
gaal --version
gaal index backfill --engine hermes
gaal ls --engine hermes -H --limit 5
gaal inspect <known-hermes-id> -H
gaal transcript <known-hermes-id>
gaal create-handoff <known-hermes-id> --dry-run -H
```

Installation should preserve the existing Hermes runtime:

- no service restarts unless explicitly authorized
- no edits to Hermes config
- no raw private Hermes data copied into this repo
- no production or LaunchAgent changes unless the user explicitly extends scope

If installation needs dependencies or PATH changes, stop and report the exact proposed mutation before applying it unless already authorized by the goal.

## Acceptance Report

Final report should include:

- branch name and commit hash
- files changed
- fixture provenance and sanitization summary
- local verification commands and results
- private-host install commands and results, if authorized
- remaining risks
- exact follow-up if live verification is blocked

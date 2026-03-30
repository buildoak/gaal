# Architecture

## High-Level Design

`gaal` is intentionally not a process monitor. It reads artifacts, indexes them, and answers questions about completed or in-progress sessions based on what is on disk.

Key components:

- Parser: dual Claude/Codex JSONL parsing
- SQLite: canonical structured store for sessions, facts, handoffs, and tags
- Tantivy: full-text search over indexed facts
- Markdown renderer: transcript generation
- Salt-based discovery: self-identification

## Module Structure

```text
src/
  main.rs              CLI entry point (clap derive)
  lib.rs               Crate root, re-exports
  config.rs            Config loading (~/.gaal/config.toml)
  error.rs             AX-compliant error types with format_human()
  util.rs              Shared utilities

  commands/            One file per command
    ls.rs              Fleet view over indexed sessions
    inspect.rs         Session detail view
    who.rs             Inverted attribution queries
    search.rs          BM25 full-text search via Tantivy
    recall.rs          Ranked handoff retrieval for session continuity
    transcript.rs      Markdown transcript rendering
    handoff.rs         LLM-powered handoff generation (create-handoff)
    salt.rs            Unique token generation for self-identification
    find.rs            JSONL file discovery by salt token (find-salt)
    index.rs           Index maintenance (backfill, reindex, prune, import-eywa, status)
    tag.rs             Tag management (add, remove, ls)
    runtime.rs         Shared command runtime (DB handle, config, output mode)
    mod.rs             Module declarations

  parser/              Dual-format JSONL parsing
    claude.rs          Claude session JSONL parser
    codex.rs           Codex session JSONL parser
    facts.rs           Unified fact extraction (tool counting, error dedup, peak context)
    common.rs          Shared parser types and utilities
    event.rs           Parsed event types
    types.rs           Parser type definitions
    mod.rs

  db/                  Persistence layer
    schema.rs          SQLite schema, autocommit guard, column migrations
    schema.sql         DDL statements
    queries.rs         All SQL queries
    mod.rs

  discovery/           JSONL file discovery on disk
    claude.rs          Claude project tree scanner (~/.claude/projects/)
    codex.rs           Codex session tree scanner (~/.codex/)
    discover.rs        Unified discovery orchestrator
    process.rs         Process-related utilities
    mod.rs

  model/               Domain types
    session.rs         SessionRow, SessionType
    fact.rs            FactRow, FactType
    handoff.rs         HandoffRow
    mod.rs

  output/              Formatting
    json.rs            JSON serialization for agent consumption
    human.rs           Human-readable tables and cards (-H mode)
    mod.rs

  render/              Markdown generation
    session_md.rs      JSONL events to markdown (frontmatter, turns, subagent summaries)
    mod.rs

  subagent/            Subagent support
    discovery.rs       Subagent JSONL file discovery (subagents/agent-*.jsonl)
    parent_parser.rs   Extract subagent metadata from parent toolUseResult blocks
    engine.rs          Subagent indexing engine
    mod.rs
```

## Data Flow

```text
JSONL files on disk
  -> discovery/
  -> parser/
  -> db/
  -> commands/
  -> output/ or render/
```

The indexing and query path is: JSONL files on disk -> discovery/ -> parser/ -> db/ -> commands/ -> output/ or render/.

## Data Model

### Sessions Table

The `sessions` table is the canonical row per indexed session. Core fields:

- `id`
- `engine`
- `model`
- `cwd`
- `started_at`
- `ended_at`
- `last_event_at`
- `parent_id`
- `session_type`
- `jsonl_path`
- token totals
- `total_tools`
- `total_turns`
- `peak_context`
- `last_indexed_offset`

### Session Types

- `standalone`: normal session with no subagents
- `coordinator`: parent session that spawned subagents via the Agent or Task tool flow
- `subagent`: child session linked to a coordinator

### Facts Table

The `facts` table stores normalized activity used by inspect, attribution, and search. Fact types:

- `file_read`
- `file_write`
- `command`
- `error`
- `git_op`
- `user_prompt`
- `assistant_reply`
- `task_spawn`

### Handoffs Table

The `handoffs` table stores generated continuity artifacts. Core fields:

- `headline`
- `projects`
- `keywords`
- `substance`
- `duration_minutes`
- `generated_at`
- `generated_by`
- `content_path`

### Tags

Session tags live in `session_tags` and are managed through the `tag` command.

### FTS Index

Tantivy indexes facts with these main fields:

- `session_id`
- `engine`
- `turn`
- `fact_type`
- `subject`
- `detail`
- `ts`
- `session_headline`

## Two-Source Subagent Model

Subagent support is intentionally split across two sources:

1. DB-backed session metadata and indexed facts. This is the fast query surface for fleet views, inspect, who, recall, tags, and most session metadata.
2. Filesystem discovery of parent JSONL plus `subagents/agent-*.jsonl`. This is the detail store used to recover raw trace detail, render transcripts, and inspect subagent turn-by-turn behavior.

For Claude coordinator sessions:

- Parent JSONL `toolUseResult` blocks provide `agentId`, duration, total tokens, status, and prompt or description.
- Subagent JSONL files provide the full turn-by-turn trace and tool activity.

The path is deterministic:

`Parent JSONL -> toolUseResult.agentId -> {session_dir}/subagents/agent-{agentId}.jsonl`

This split exists because the database is optimized for fast structured queries, while the raw files remain the authoritative detail source for transcript rendering, salt discovery, and subagent trace recovery.

## Codex Subagent Model

Codex subagents do not use Claude's `toolUseResult.agentId -> subagents/agent-*.jsonl`
layout. The child is its own top-level rollout JSONL under `~/.codex/sessions/...`, and the
parent-child relationship is reconstructed from metadata embedded in both the child and parent
session streams.

The child rollout is identified from its own `session_meta` record. Real Codex child sessions
carry both a canonical `forked_from_id` field and a richer `source.subagent` block:

```json
{
  "type": "session_meta",
  "payload": {
    "id": "019d261e-6e93-78d0-8f2c-29279b9e8252",
    "forked_from_id": "019d261d-dffa-7d21-b0df-5893b4ca9aaf",
    "source": {
      "subagent": {
        "thread_spawn": {
          "parent_thread_id": "019d261d-dffa-7d21-b0df-5893b4ca9aaf",
          "agent_role": "explorer",
          "agent_nickname": "Schrodinger"
        }
      }
    }
  }
}
```

For indexing, `forked_from_id` is the canonical linkage key. It is cheap to discover from the
file head, stable across the pipeline, and maps directly onto the session row fields
`session_type = 'subagent'` and `parent_id = truncate_codex_id(forked_from_id)`. The
`source.subagent` block is still useful as corroborating evidence that the session is a spawned
Codex child and as a source of role or nickname context, but parent-child linking does not depend
on parsing that nested object.

Parent-child linking in the Codex backfill pipeline happens in three stages:

1. Discovery scans `~/.codex/sessions` for `rollout-*.jsonl` files and reads the file head.
   If `forked_from_id` is present, the discovered session is marked as a child candidate before
   full parsing starts.
2. Indexing parses the session and writes the child row immediately. `apply_codex_subagent_link()`
   sets `session_type = 'subagent'`, stores the truncated parent ID in `parent_id`, and carries
   `agent_role` into `subagent_type` when present.
3. Coordinator promotion runs after session rows exist. Any Codex session whose short ID appears
   as a `parent_id` on one or more child rows is promoted from `standalone` to `coordinator`.

That ordering matters. Codex does not require the parent rollout to be processed first. A child
can be indexed with `parent_id` already populated even if the parent row has not been seen yet.
Promotion is a second pass over the indexed rows, so the parent becomes a coordinator once its own
session appears in the database and its short ID matches one or more children.

This differs from Claude's Agent tool model. Claude coordinator sessions expose subagent metadata
through parent-side `toolUseResult` blocks, where `agentId` is the durable key and the subagent
trace lives at a deterministic child path:

`Parent JSONL -> toolUseResult.agentId -> {session_dir}/subagents/agent-{agentId}.jsonl`

Codex does not have that file layout or that identifier path. Instead:

- Child identity comes from the child's own `session_meta`, especially `forked_from_id`.
- Parent fleet metadata comes from parent `response_item` records containing `spawn_agent`,
  `wait_agent`, and `close_agent` function calls and outputs.

In other words, Claude is parent-first for identity and summary metadata, while Codex is child-first
for identity and parent-assisted for summary metadata.

The Codex implementation therefore uses a two-source pattern specific to Codex:

1. Child `session_meta` in the child's own JSONL provides identity. This is where `id`,
   `forked_from_id`, `agent_role`, and `agent_nickname` originate.
2. Parent function-call history provides fleet metadata. `spawn_agent` yields the dispatched
   prompt and `agent_type`; `close_agent` yields terminal status; `wait_agent` records lifecycle
   progress even though it is not the primary identity source.

The practical split is:

- Use the child JSONL when the question is "who is this child attached to?"
- Use the parent JSONL when the question is "what was this child asked to do and how did it end?"

That matches the current parser and indexer. `extract_codex_spawn_summaries()` walks the parent
rollout, pairs `spawn_agent` function calls with their outputs to recover the spawned `agent_id`,
copies the parent prompt into `SubagentMeta.prompt`, records `agent_type` as `subagent_type`, and
later updates `status` when the matching `close_agent` output arrives.

Two edge cases are important:

- If `forked_from_id` is absent, the session is indexed as `standalone`. No Codex child link is
  inferred from parent-side tool traffic alone.
- If the child is indexed before the parent exists in the database, the child still keeps its
  `parent_id`. Coordinator linking is deferred until the parent session is indexed and the
  promotion pass can flip that parent row to `session_type = 'coordinator'`.

This keeps the Codex model resilient to out-of-order discovery while preserving the same core rule
as the Claude model: identity comes from the most authoritative source, and descriptive metadata is
merged in from the complementary source that actually records it.

## Session Lifecycle

The common operator workflow is:

1. `gaal index backfill`
2. `gaal inspect latest`
3. `gaal who <verb> <target>` or `gaal search <query>`
4. `gaal transcript <id>`
5. `gaal create-handoff <id>`
6. `gaal recall <topic>`

The self-handoff flow is:

1. Run `gaal salt`
2. Run `gaal find-salt <token>`
3. Run `gaal create-handoff --jsonl <path>`

This flow depends on salt-based self-identification so an in-progress session can locate its own JSONL on disk before generating a handoff.

## Token Accounting

Token accounting is parser-driven and model-aware:

- Cache tokens are fully tracked as `cache_read_tokens` and `cache_creation_tokens`.
- Peak context is the maximum of `input_tokens + cache_read_tokens + cache_creation_tokens` across all counted turns.
- Model-aware cost estimation uses per-model pricing instead of one flat rate.
- Tool counting includes both Claude and Codex tool uses.
- Usage deduplication differs by engine: Claude uses `dedup_key`, while Codex uses cumulative `total_tokens`.
- Error deduplication uses `tool:{tool_use_id}` when available, otherwise `ts:{timestamp}|exit:{code}`.

## Storage Layout

```text
~/.gaal/
  index.db
  tantivy/
  config.toml
  data/
    claude/
      sessions/YYYY/MM/DD/<id>.md
      handoffs/YYYY/MM/DD/<id>.md
    codex/
      sessions/YYYY/MM/DD/<id>.md
      handoffs/YYYY/MM/DD/<id>.md
```

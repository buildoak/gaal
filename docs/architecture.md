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

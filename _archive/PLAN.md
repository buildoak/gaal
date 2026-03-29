<!-- Archived 2026-03-29: superseded by DOCS.md / BACKLOG.md -->
---
date: 2026-03-03
engine: coordinator
status: in-progress
---

# Gaal Build Plan

## Module Structure

```
gaal/
  Cargo.toml
  DESIGN.md
  PLAN.md
  src/
    main.rs                    # clap CLI, dispatch to commands
    lib.rs                     # re-exports all modules
    error.rs                   # GaalError enum, exit codes
    config.rs                  # ~/.gaal/config.toml loading
    db/
      mod.rs                   # SQLite connection, init, migrate
      schema.sql               # DDL (embedded via include_str!)
      queries.rs               # typed query functions
    parser/
      mod.rs                   # detect_engine + unified parse entry
      claude.rs                # Claude JSONL -> Vec<Fact>
      codex.rs                 # Codex JSONL -> Vec<Fact>
      types.rs                 # RawEvent, ParsedSession, engine detection
    discovery/
      mod.rs                   # discover_sessions (all engines)
      claude.rs                # Claude JSONL path patterns
      codex.rs                 # Codex JSONL path patterns
      active.rs                # PID probing, tmux detection, process stats
    model/
      mod.rs                   # re-exports
      session.rs               # SessionRecord (matches JSON output schema)
      fact.rs                  # Fact (matches facts table)
      handoff.rs               # HandoffRecord
      status.rs                # Status enum, StuckSignals, status computation
    commands/
      mod.rs                   # re-exports all commands
      ls.rs                    # gaal ls
      show.rs                  # gaal show
      inspect.rs               # gaal inspect
      who.rs                   # gaal who
      search.rs                # gaal search (Tantivy)
      recall.rs                # gaal recall (IDF + recency)
      handoff.rs               # gaal handoff (shell out to agent-mux)
      active.rs                # gaal active
      index.rs                 # gaal index backfill/status/reindex/import-eywa/prune
      tag.rs                   # gaal tag
    output/
      mod.rs                   # OutputFormat enum, serialize helpers
      json.rs                  # JSON serialization (default)
      human.rs                 # -H human-readable table formatting
  tests/
    integration/
      parse_claude.rs          # Real JSONL parsing
      parse_codex.rs           # Real JSONL parsing
      db_queries.rs            # SQLite query tests
      discovery.rs             # Session discovery tests
```

## Dependencies

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
thiserror = "2"
rusqlite = { version = "0.32", features = ["bundled"] }
tantivy = "0.22"
toml = "0.8"
dirs = "5"
```

## Phase 1 Workers (Foundation)

### W1: Cargo.toml + module skeleton
- Create Cargo.toml with all deps
- Create all .rs files with `mod` declarations only
- Ensure `cargo check` passes (empty modules)

### W2: error.rs + config.rs + model types
- GaalError enum with exit codes (0, 1, 2, 3, 10, 11)
- Config struct for config.toml
- SessionRecord, Fact, HandoffRecord, Status enum, StuckSignals
- All types derive Serialize/Deserialize

### W3: db/schema.sql + db/mod.rs + db/queries.rs
- Embed schema.sql via include_str!
- init_db() creates tables + indexes
- Typed query functions for sessions, facts, handoffs, tags

### W4: parser/types.rs + parser/mod.rs + parser/claude.rs + parser/codex.rs
- Port Orac parser logic, adapted for Gaal's Fact extraction
- detect_engine from first 10 lines
- parse_session returns (SessionMeta, Vec<Fact>)
- Extract: file_read, file_write, command, error, git_op, user_prompt, assistant_reply, task_spawn

### W5: discovery/mod.rs + discovery/claude.rs + discovery/codex.rs
- Discover all JSONL files for each engine
- Return Vec<DiscoveredSession> with path, engine, id, basic meta

### W6: discovery/active.rs
- PID probing via kill -0
- Process stats (CPU, RSS) via ps
- tmux session detection
- JSONL path resolution from PID/CWD

### W7: output/mod.rs + output/json.rs + output/human.rs
- OutputFormat enum (Json, Human)
- Serialize any Serialize type to JSON or human table
- -H flag support

## Phase 2 Workers (Verbs)

Each worker gets: relevant DESIGN.md section, types from Phase 1, test requirements.

### W8: commands/index.rs (backfill + status + reindex + import-eywa + prune)
### W9: commands/ls.rs
### W10: commands/show.rs
### W11: commands/who.rs
### W12: commands/active.rs
### W13: commands/inspect.rs
### W14: commands/recall.rs (port eywa scoring)
### W15: commands/search.rs (Tantivy)
### W16: commands/handoff.rs (shell out to agent-mux)
### W17: commands/tag.rs
### W18: main.rs (full clap CLI wiring)

## Test Matrix

| Test | Proves |
|------|--------|
| Parse real Claude JSONL | Parser extracts correct facts, tokens, model |
| Parse real Codex JSONL | Parser handles event-driven format |
| Engine detection | detect_engine correctly identifies both |
| DB init + migrate | Schema creates cleanly |
| Insert + query sessions | Round-trip through SQLite |
| Insert + query facts | Fact types, subject matching |
| Session discovery | Finds JSONL in expected paths |
| ls with filters | Status, engine, since, before, stuck filters |
| show with flags | Files, errors, commands, git, tokens, tree |
| who verb expansion | installed/deleted expand correctly |
| recall scoring | IDF + recency produces expected ranking |
| search indexing | Tantivy indexes and queries facts |

## Port vs Rewrite Decisions

| Component | Decision | Reason |
|-----------|----------|--------|
| Claude parser | Port + adapt | Orac's logic is solid, add Fact extraction |
| Codex parser | Port + adapt | Same |
| Engine detection | Port directly | 10-line heuristic, works as-is |
| Discovery paths | Port + extend | Add PID probing, trim TUI concerns |
| Tantivy search | Port + adapt | Change schema to facts, not sessions |
| Cost calculation | Port directly | Simple, correct |
| Eywa scoring | Rewrite in Rust | Python -> Rust, same algorithm |
| Session-detect | Rewrite in Rust | Bash/Python -> Rust, same approach |
| TUI | Skip entirely | Gaal is CLI-only |

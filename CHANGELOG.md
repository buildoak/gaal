# Changelog

## 2026-03-31

### Fixed
- **Filter agent-*.jsonl from session discovery** — subagent files were being discovered as top-level sessions, causing perpetual re-indexing of ~677 sessions every backfill run (`src/discovery/claude.rs`)

### Performance
- **Batch-load codex invalid-error session IDs** instead of per-session SQL query — drops steady-state backfill from 70s to <1s (`src/commands/index/mod.rs`)
- **Skip Tantivy search index rebuild** when no sessions were indexed (`src/commands/index/mod.rs`)

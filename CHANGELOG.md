# Changelog

## 2026-04-16

### Fixed
- **UTF-8 panic in `index backfill` render pipeline** when session transcripts contained multi-byte codepoints in bash command strings. Two byte-index slicing sites in `src/render/session_md.rs` (truncation limits 57 and 37) would panic on non-ASCII bytes at the cut boundary. Replaced with codepoint-safe `chars().take(N)`. Regression test added.

### Performance
- **`index backfill` is now incremental.** Per-engine mtime cursors stored in a new `meta` SQLite table (`backfill:claude`, `backfill:codex`, `backfill:gemini`) gate discovery — files whose on-disk mtime is older than `cursor - 10s` are skipped before any head-read, JSON parse, or SQLite lookup. A 10-second safety margin covers actively-appending files. Cursors advance only on successful per-engine passes; a stalled engine leaves its cursor untouched so the next run retries the missed window, and other engines still advance independently. First run (no cursor) and DB wipes fall through to the existing full-scan baseline. `--since`, `--engine`, and `--force` flags still work — the mtime gate is additive. Replaces the previous behavior that walked all ~6,784 sessions every run.

## 2026-04-06

### Added
- **Gemini CLI engine support** — gaal now discovers, parses, indexes, and renders Gemini CLI sessions (`~/.gemini/tmp/*/chats/session-*.json`). New `src/parser/gemini.rs` and `src/discovery/gemini.rs`. 145 sessions indexed on first backfill
- **`--engine gemini` filter** — all query subcommands (`ls`, `inspect`, `who`, `search`, `recall`) accept `--engine gemini` to scope results
- **Gemini extended thinking (Thoughts)** — thought blocks stored and rendered in transcripts
- **Tool name normalization** — Gemini tool names mapped to canonical gaal names
- **`gemini_summary` field** — sessions table gains a Gemini-specific summary column
- **Info/warning/error message type parsing** — Gemini message types properly classified
- **Incremental indexing for Gemini** — file mtime+size gating, full re-parse on change

## 2026-03-31

### Fixed
- **Filter agent-*.jsonl from session discovery** — subagent files were being discovered as top-level sessions, causing perpetual re-indexing of ~677 sessions every backfill run (`src/discovery/claude.rs`)

### Performance
- **Batch-load codex invalid-error session IDs** instead of per-session SQL query — drops steady-state backfill from 70s to <1s (`src/commands/index/mod.rs`)
- **Skip Tantivy search index rebuild** when no sessions were indexed (`src/commands/index/mod.rs`)

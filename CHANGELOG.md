# Changelog

## Unreleased

### Added
- **Source-backed activity slices.** New `gaal activity` command renders transcript-shaped markdown for sessions with source-proven events in a half-open time window. Activity output lives under `~/.gaal/data/activity/` and includes carried-in/continues-after metadata for long-running sessions.

### Fixed
- **Transcript-aware `create-handoff` planning.** Handoff chunk planning now sizes the actual rendered transcript used for extraction instead of raw Codex JSONL bytes. This prevents short rendered sessions with large JSONL tool payloads from exploding into unnecessary map/reduce calls, and makes long rendered transcripts chunk predictably before dispatch.

## 2026-05-05 — 0.3.0

### Added
- **Hermes Agent engine support.** Gaal now discovers, parses, indexes, searches, resolves, renders transcripts for, and dry-run plans handoffs for Hermes sessions from `~/.hermes/state.db`.
- **Hermes transcript and handoff paths use full session IDs.** Hermes date-prefixed IDs collide under first-8 aliases, so derived artifacts preserve the full sanitized Hermes ID.
- **Hermes sanitized SQLite fixtures.** Local tests cover CLI, Telegram, cron, tool-call, and parent-linked continuation session shapes without committing private Hermes content.
- **Hermes scheduling path.** A user LaunchAgent can run `gaal index backfill --engine hermes --with-markdown` every minute without touching Hermes services or config.

### Fixed
- **Hermes tool result rendering.** Hermes transcripts now include generic `tool` result bodies with truncation instead of only showing tool-call annotations.

### Changed
- **Release version bumped to 0.3.0.**

## 2026-05-05 — 0.2.0

### Added
- **Automatic chunked `create-handoff` generation for long sessions.** Long Codex sessions now use compaction checkpoints as map/reduce boundaries, while other long transcripts fall back to turn/line splitting. The CLI remains low-friction: `gaal create-handoff <id>` chooses the correct path automatically and still writes one final handoff markdown file plus one DB row.
- **Coverage manifests in generated handoffs.** Chunked handoffs now record source JSONL line coverage, compaction boundaries, mapper/reducer call counts, source used, rendered transcript line ranges, and confirm that no surfaced `.parts` files were created.
- **Single-session dry-run planning.** `gaal create-handoff <id> --dry-run` now reports strategy, chunk count, estimated calls, compaction lines, provider/model/effort, planned handoff path, and explicit side-effect booleans.

### Fixed
- **Dry-run safety for `create-handoff`.** Single-session dry runs no longer invoke providers, spend tokens, write handoff markdown, upsert DB rows, or index unindexed JSONL files.
- **Fresh transcript rendering for handoff generation.** Handoff creation now prefers rendering directly from JSONL so active or recently updated sessions do not accidentally use stale cached markdown.
- **Large prompt dispatch stability.** Agent-mux handoff calls now use prompt files instead of passing giant contexts through argv.
- **Stale session cwd handling.** If an indexed session cwd no longer exists, handoff generation falls back to the current working directory instead of failing inside agent-mux startup.
- **Local-date frontmatter.** Handoff frontmatter dates now match the rendered transcript’s local date for sessions that cross UTC day boundaries.

### Changed
- **Provider honesty.** `--provider openrouter` remains visible in dry-run planning as unsupported for real execution instead of silently routing through agent-mux.
- **Handoff fidelity prompts.** Chunk mapper/reducer prompts now explicitly preserve absolute paths, dirty worktree state, verification evidence, and continuation-critical risks.
- **Release version bumped to 0.2.0.**

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

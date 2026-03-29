# BACKLOG.md — Future Features & Design Notes

## Incremental Parsing (SHA-256 prefix + byte offset resume)

**Source:** tokscale v2.0.0 (`crates/tokscale-core/src/message_cache.rs` + `sessions/codex.rs`)
**Priority:** P3 — would make `gaal index backfill` near-instant for append-only sessions

### Problem

`gaal index backfill` currently has two skip paths:

1. **Size-based skip** (`last_indexed_offset == file_size`): the session is completely untouched since last run. Skipped, no I/O.
2. **Incremental parse** (`last_indexed_offset < file_size`): gaal reads from the stored byte offset forward, parses only the new lines, and merges the delta into the existing `SessionRow`. This is already implemented in `parse_session_incremental()`.

What gaal does **not** do is verify that the bytes *before* the stored offset have not changed. If a session file is rewritten from the start (tool update, log rotation, partial truncation), gaal will silently parse garbage from mid-file and accumulate incorrect token counts. With 4000+ sessions and growing, a single corrupted resume also goes undetected until `--force` is run manually.

The problem tokscale solved is the *trust* problem: how do you know the prefix you're skipping is still the same prefix?

### How tokscale does it

tokscale's answer lives across two modules: `message_cache.rs` (fingerprint infrastructure) and `sessions/codex.rs` (incremental state machine). They compose into a layered system.

#### SourceFingerprint — the multi-layered identity check

`message_cache.rs` defines a `SourceFingerprint` that captures file identity at multiple granularities:

```rust
struct SourceFingerprint {
    file_size: u64,
    mtime: SystemTime,
    sample_hashes: [u64; 5],   // xxhash64 of 5 byte-range samples spread across the file
    sha256_prefix: [u8; 32],   // SHA-256 of the first N bytes (configurable, default 64KB)
}
```

The layers serve different cost/confidence tradeoffs:

- **`file_size` + `mtime`**: nearly free, rules out most untouched files immediately
- **`sample_hashes`**: five 64-bit hashes of evenly-spaced 4KB chunks across the whole file — catches content changes that preserve size and mtime (e.g. in-place rewrites)
- **`sha256_prefix`**: the definitive check — hashes the first 64KB of the file to confirm the already-parsed prefix is unchanged

Fingerprints are serialized with `bincode` into a persistent on-disk cache at `~/.tokscale/source-cache/` (one file per session, named by a hash of the source path). Atomic writes via a temp-file + rename pattern prevent corruption. On concurrent access, a `fs2` advisory lock prevents two processes from writing the same cache entry simultaneously.

The WAL sidecar awareness mentioned in the dissection refers to tokscale skipping SQLite WAL files (`.db-wal`, `.db-shm`) from its cache so it doesn't accidentally fingerprint transient write-ahead log state.

#### CodexIncrementalCache — the stateful byte offset resume

`sessions/codex.rs` contains the actual incremental parser state. The key struct:

```rust
struct CodexIncrementalCache {
    last_byte_offset: u64,
    last_token_usage: TokenBreakdown,  // cumulative totals at last-indexed position
    fingerprint: SourceFingerprint,
}
```

**Resume decision logic (simplified):**

```
1. Load CodexIncrementalCache from disk (if any)
2. Compute current SourceFingerprint for the file
3. If fingerprint.file_size < cache.last_byte_offset:
       → file shrank (truncation) → full reparse, invalidate cache
4. If fingerprint.sha256_prefix != cache.fingerprint.sha256_prefix:
       → prefix changed → full reparse, invalidate cache
5. Else if fingerprint.file_size == cache.fingerprint.file_size:
       → file unchanged → return cached result, skip all I/O
6. Else (file grew, prefix still matches):
       → seek to last_byte_offset
       → read from offset to EOF
       → verify new lines start on a valid JSONL newline boundary
       → parse delta lines only
       → apply monotonic regression check on token totals
       → update cache with new offset + new totals + new fingerprint
```

**Newline boundary check:** before trusting the offset, tokscale reads the byte at `last_byte_offset - 1` and asserts it is `\n`. If it is not, the offset points mid-record — stale regression, fall back to full reparse. This guards against any byte-counting discrepancy between the writer and the cache.

**Stale regression handling:** the Codex parser tracks `last_token_usage` (the cumulative token count at the last indexed position). After reading the delta, if the first delta line's cumulative total is *less than* the stored total, the stream was rewritten — full reparse. Zero-token snapshots are skipped entirely so repeated heartbeat lines don't corrupt the accumulator.

**Concurrent writer merging:** if two tokscale processes both finish a cache update for the same source file, the one that wrote last wins (last-write-wins on atomic rename). Because fingerprints include the full SHA-256 prefix, a stale merged entry will be caught and discarded on the next run.

### Adaptation for gaal

gaal already implements the easy half: byte-offset-based incremental parsing. `parse_session_incremental()` in `src/parser/mod.rs` seeks to `last_indexed_offset`, parses forward, and returns a delta `ParsedSession`. `build_incremental_session_row()` in `src/commands/index.rs` accumulates the delta token counts into the existing `SessionRow`. The `sessions` table already has `last_indexed_offset INTEGER` as the resume cursor.

What gaal is missing is the prefix trust layer. Here is the direct mapping:

| tokscale concept | gaal adaptation |
|---|---|
| `SourceFingerprint` | New struct in `src/parser/common.rs` or a dedicated `src/parser/fingerprint.rs` |
| `sha256_prefix` (first 64KB SHA-256) | Same — add `sha2` crate (already in tokscale's Cargo.toml, cheap to add) |
| `sample_hashes` (5 xxhash samples) | Optional — gaal could start with just size + mtime + SHA-256 prefix; add sample hashes if mtime proves unreliable on network filesystems |
| Per-file on-disk cache (`~/.tokscale/source-cache/`) | Store the SHA-256 prefix alongside `last_indexed_offset` in the `sessions` table — no separate cache file needed |
| `CodexIncrementalCache.last_byte_offset` | Already exists as `sessions.last_indexed_offset` |
| Stale regression check on token totals | Add to `build_incremental_session_row()`: if `parsed_delta` total tokens are negative when merged with existing, trigger full reparse |
| Newline boundary check before trusting offset | Add to `parse_session_incremental()`: assert byte at `offset - 1` is `b'\n'` before reading from `offset` |
| Atomic cache writes with fs2 lock | gaal uses SQLite WAL — updates to `sessions` are already atomic via savepoints (see `index_session` savepoint in `index_discovered_session()`) |

**Concrete changes required:**

1. **Schema migration** (`src/db/schema.sql` + `src/db/schema.rs`): add `sha256_prefix TEXT` column to `sessions`. The `init_db()` migration pattern already handles additive column additions with `ALTER TABLE ... ADD COLUMN` guarded by `pragma_table_info` checks — follow the same pattern.

2. **Fingerprint computation** (new `src/parser/fingerprint.rs`): implement `compute_sha256_prefix(path, limit_bytes) -> [u8; 32]` using the `sha2` crate. Read at most 64KB. Return the digest.

3. **Fingerprint storage** (`src/db/queries.rs`): extend `SessionRow` with `sha256_prefix: Option<String>` (hex-encoded). Update `upsert_session()` to persist it. Populate it during full parse (`build_full_session_row()` in `index.rs`) and recompute + re-store during successful incremental parse.

4. **Trust gate in `index_discovered_session()`** (`src/commands/index.rs`): before calling `parse_session_incremental()`, compute the current file's SHA-256 prefix and compare against `existing_row.sha256_prefix`. Mismatch → fall through to full reparse path. Match + file grew → proceed with incremental.

5. **Newline boundary check** (`src/parser/claude.rs`, `src/parser/codex.rs`): in `parse_events_from_offset()`, before seeking, open the file, seek to `offset - 1`, read one byte, assert `b'\n'`. On failure, return `Err` so the caller falls back to full reparse.

6. **Stale regression guard** (`src/commands/index.rs`): in `build_incremental_session_row()`, check that accumulated totals are non-decreasing. If any token field goes negative after merge, return `Err` to trigger full reparse.

**What does NOT need to change:**

- The Tantivy FTS index rebuild (`search::build_search_index()`) already runs after all sessions are processed in `run_backfill()` — no change needed there.
- The `--force` flag already bypasses all skip/incremental logic by setting `force: true`, which disables the size-check and forces full reparse. This becomes the escape hatch when fingerprints mismatch in unexpected ways.
- The `gaal index reindex <id>` command already does a full reparse of a single session — no change needed.

### Estimated effort

3-5 days.

- Day 1: Add `sha2` dependency, implement `fingerprint.rs`, schema migration for `sha256_prefix` column.
- Day 2: Wire fingerprint into `index_discovered_session()` trust gate. Update `upsert_session()` + `SessionRow`.
- Day 3: Add newline boundary check in `parse_events_from_offset()` for both Claude and Codex parsers.
- Day 4: Add stale regression guard in `build_incremental_session_row()`. Write tests covering: file unchanged, file grew (valid resume), file prefix changed (should fall back), file truncated (should fall back), mid-record offset (should fall back).
- Day 5: Integration test with real session corpus, benchmark `gaal index backfill` before/after on 4000+ sessions.

The core tokscale pattern (`SourceFingerprint` + `sha256_prefix` check before trusting offset) ports cleanly because gaal's existing `last_indexed_offset` column is already the byte cursor. The main difference is that tokscale stores its cache in a separate directory of `bincode` files while gaal can store the fingerprint directly in `sessions.sha256_prefix`, eliminating the file-per-session cache layer entirely.

---

## Subagent First-Class Support

**Priority:** P0 — critical path. Three capabilities blocked: `who` attribution, transcript subagent summaries (broken since CC v2.1.86), and parent→child inspect drill-down.
**Date:** 2026-03-29 (revised from 2026-03-28 draft)

### Shipped (2026-03-29)

- `src/subagent/` module (`discovery.rs`, `parent_parser.rs`, `engine.rs`) — 237 lines
- DB indexing: 5,170 subagents, 174 coordinators
- `gaal ls --include-subagents`
- `gaal inspect` shows Subagents table for coordinators, `parent_id` for subagents
- `gaal search` includes subagent content in Tantivy
- `gaal transcript` DB-backed subagent data (replaces dead `SubagentProgress` pipeline)
- Facts extraction fix: inline `ContentBlock::ToolUse` now generates `file_read`/`file_write`/`command` facts (was missing — 136K new facts)
- Transcript DB lookup fix: `.or_else()` on `Some(vec![])` fallback repaired

### Open Issues (2026-03-29)

- Duplicate entries: 2,253 8-char orphans + 12-char linked pairs (fix in progress)
- Transcript Task column blank for v2.1.86+ sessions (prompt not pulled from facts)
- `ls` output missing `session_type` field
- `who` output missing parent→subagent attribution format
- Positional model mapping in transcript (fragile, should match by `agent_id`)
- 4,051 orphan subagent files from CC's 30-day cleanup (historical loss, not recoverable without parent JSONL)

### Why This Is Urgent

Claude Code v2.1.86 (2026-03-28) stopped emitting `SubagentProgress` events. The transcript renderer's subagent summary table, Files Touched by Subagents, and Subagent Activity sections are **permanently broken for all new sessions.** This is not a regression we can wait on — every coordinator session recorded from now on has zero subagent metadata in its transcript.

Additionally, `gaal who` — the killer feature — is blind to subagent file activity. 7,029 subagent JSONLs (1.55 GB) containing the actual work product are invisible to search, who, inspect, and recall.

### Data Architecture (Verified 2026-03-29)

**Two data sources, complementary roles:**

| Source | Role | What it provides |
|--------|------|-----------------|
| Parent JSONL `toolUseResult` blocks | **Fleet index** | agentId, totalTokens, totalDurationMs, totalToolUseCount, status, prompt, final output text |
| Subagent JSONL files (`subagents/agent-{agentId}.jsonl`) | **Detail store** | Full conversation, every tool call, every file read/write, per-turn token usage |

**Dead end — do NOT build on:**
- `SubagentProgress` events — deprecated by CC v2.1.86+. Use only as legacy fallback for pre-v2.1.86 sessions.

**Path from parent to subagent file is deterministic:**
Parent JSONL → `toolUseResult.agentId` → `{session_dir}/subagents/agent-{agentId}.jsonl`

### Schema (Already Exists)

No DDL migration needed. All columns and indexes already exist in the `sessions` table:
- `parent_id TEXT REFERENCES sessions(id)` + index
- `session_type TEXT DEFAULT 'standalone' CHECK(session_type IN ('standalone', 'coordinator', 'subagent'))` + index
- `facts` table supports `task_spawn`, `file_read`, `file_write`, `command` fact types

Only Rust struct changes needed: add `parent_id: Option<String>` to `SessionRow`, update `upsert_session()` SQL.

### Target AX

**`gaal who read src/render/session_md.rs`**
```
  7d5d03e4  2026-03-28  claude-opus-4-6     → a59e6762 (Fix Agent rendering in transcripts)
```
Attribution flows through parent to the subagent that did the work.

**`gaal inspect <parent-id>`** — shows Subagents (N) table:
```
  Subagents (34):
  ID        Model          Tokens    Duration  Description
  a59e6762  sonnet-4-6     75K       4m 47s    Fix Agent rendering in transcripts
  a930b582  sonnet-4-6     78K       5m 35s    Investigate context_tokens and caveat title
```
Data comes from parent's `toolUseResult` blocks — no subagent JSONL read needed for this view.

**`gaal inspect <subagent-id>`** — same as any session:
```
  Session: a59e6762 (subagent of 7d5d03e4)
  Model: sonnet-4-6
  Task: "Fix Agent rendering in transcripts"
  Files read: session_md.rs, CLAUDE.md, BACKLOG.md
  Files written: session_md.rs
  Commands: cargo build --release, gaal transcript 7d5d03e4 --stdout
```

**`gaal search`, `gaal recall`** — subagent facts in the same Tantivy index, transparent.

**`gaal ls`** — subagents hidden by default. `--include-subagents` or `--type subagent` to show.

**`gaal transcript`** — renderer pulls subagent summary from DB instead of dead SubagentProgress events.

### Implementation Phases

#### Phase 1: Discovery + Parent-Side Indexing (2 days)

**Goal:** Parse `toolUseResult` from parent JSONLs. Populate `sessions` table with subagent rows using fleet-level metadata. No subagent JSONL reads yet.

1. `src/parser/facts.rs` or `src/parser/claude.rs`: Detect `toolUseResult` on user events. Extract `agentId`, `totalTokens`, `totalDurationMs`, `totalToolUseCount`, `status`, `prompt`.
2. `src/db/queries.rs`: Add `parent_id` to `SessionRow` + `upsert_session()`.
3. `src/commands/index.rs`: For each `toolUseResult`, create a subagent `SessionRow` with `session_type = "subagent"`, `parent_id` linked, fleet-level token/duration stats. Mark parent as `session_type = "coordinator"`.
4. `src/commands/ls.rs`: Add `--include-subagents` flag. Default WHERE excludes `session_type = 'subagent'`.
5. `src/commands/inspect.rs`: When inspecting a parent, query child sessions and render Subagents (N) table.

**Verification gate:** `gaal ls` hides subagents. `gaal inspect 7d5d03e4` shows 35 subagents with IDs, models, token counts, durations. Parent shows as `session_type = "coordinator"`.

#### Phase 2: Subagent JSONL Indexing (3 days)

**Goal:** Discover and parse subagent JSONL files. Populate `facts` table with file_read, file_write, command facts. Powers `who` and `search`.

1. `src/discovery/claude.rs`: Add `collect_subagent_jsonl_files()` — scan `{session_dir}/subagents/` for `agent-*.jsonl`. Do NOT make `collect_project_jsonl_files()` recursive.
2. `src/discovery/discover.rs`: Add `SubagentInfo` to `DiscoveredSession` (agent_id, parent_session_uuid, description from .meta.json).
3. Parse subagent JSONLs through existing Claude parser — format is identical. Override session ID with agentId prefix.
4. Link subagent facts to the subagent session row created in Phase 1.

**Verification gate:** `gaal who read src/render/session_md.rs` shows subagent attribution. `gaal search "cargo build"` returns subagent sessions. `gaal inspect <subagent-id>` shows internal file reads, commands, tool counts.

#### Phase 3: Transcript Renderer Migration (1 day) — SHIPPED 2026-03-29

**Goal:** Replace dead SubagentProgress pipeline with DB-backed subagent data.

1. `src/render/session_md.rs`: In Executive Summary, query DB for child sessions instead of building SubagentDelta from progress events. ✓
2. Populate summary table from `sessions` table (parent_id query). ✓
3. Populate "Files Touched by Subagents" from subagent facts. ✓
4. Keep SubagentProgress parsing as legacy fallback for pre-v2.1.86 sessions where DB hasn't been backfilled. ✓

**Remaining gaps (see Open Issues above):** Task column blank for v2.1.86+ sessions; positional model mapping is fragile.

#### Phase 4: Polish + Edge Cases (1 day)

1. Handle orphaned subagents (parent JSONL deleted).
2. Handle zero-turn / empty subagents.
3. `gaal recall` verification — FTS fallback includes subagent content.
4. ID collision testing — agentId 8-char prefixes vs UUID 8-char prefixes across full corpus.
5. `--skip-subagents` flag for `gaal index backfill` (opt-out for speed).

### Performance Impact

- First backfill: ~7,029 files, 1.55 GB, ~30-60 seconds
- DB growth: ~7,029 new session rows + ~150K new facts
- Tantivy rebuild: ~6 seconds (up from <2 seconds)
- Subsequent runs: skip unchanged files via size-based check

### Risks

1. **ID collision:** agentId 8-char prefix could collide with UUID 8-char prefix. Test against full corpus before shipping.
2. **Agent-mux workers:** Dispatched via Bash, not Agent tool — no `toolUseResult` in parent JSONL, no subagent JSONL files. These remain invisible until new agent-mux emits proper metadata. Known P2 gap.
3. **Pre-v2.1.86 sessions:** Have SubagentProgress but may not have been backfilled. Phase 1 parsing handles both old and new formats.

---

## CC Session Cleanup Mitigation

**Priority:** P1
**Date:** 2026-03-29

### Context

CC's `cleanupPeriodDays` was set to 30 by default. On 2026-03-29, verified that 4,051 subagent JSONL files have already been pruned — their parent coordinator sessions survive but the child files are gone. These are unrecoverable without the parent JSONL (the `toolUseResult` blocks only exist in the parent, not the orphaned subagent files).

### Actions Taken

- `cleanupPeriodDays` set to **365** on 2026-03-29 — future sessions will not be pruned for a year

### Remaining Work

- **Daily `gaal index backfill` cron** (P1): Backfill must run before any future pruning window expires. A day-old session that hasn't been indexed yet is recoverable; a session pruned before indexing is gone. Add cron or LaunchAgent to run `gaal index backfill` nightly.
- **Orphan recovery for existing 4,051 files** (P2): Subagent files that still exist on disk but whose parent JSONL is gone can be indexed independently using the internal `parentUuid` field embedded in each subagent JSONL. Parse `parentUuid` from the subagent file itself to reconstruct the link. This recovers file_read/file_write/command facts and makes the subagent searchable, but fleet-level metadata (totalTokens, totalDurationMs, status, prompt) is lost.

---

## Transcript Title Caveat Leak

**Priority:** P1 — visible quality bug in transcript titles
**Date added:** 2026-03-29

### Problem

Sessions whose first user message is a system-injected `<local-command-caveat>` block render as `# Session: Caveat: The messages belo...`. The XML-stripping fix (efa6648) strips the tag but the plain text inside survives as the title. Same bug class affects `<tg_message_voice>` injection — titles show raw XML fragments.

### Root Cause

In `src/render/session_md.rs`, `get_first_user_prompt()` truncates the raw text to 47 chars BEFORE `strip_xml_tags()` is called. After truncation, the tag is complete and gets stripped — but the content ("Caveat: The messages...") is plain prose that passes through.

### Fix

Move `strip_xml_tags()` call into `get_first_user_prompt()`, apply it to full text BEFORE truncation. Then skip turns whose stripped text starts with known injection patterns ("Caveat:", empty after stripping). This handles caveat, tg_message_voice, and any future injection wrappers.

**Effort:** ~30 min. One function in `session_md.rs`, no schema changes.

---

## DOCS.md / Documentation Structure

**Priority:** P1 — gaal lacks user-facing documentation outside README
**Date added:** 2026-03-29

### Problem

Gaal has README.md (rewritten 2026-03-28 to match current command surface) and CLAUDE.md (operator guidance for AI workers). Missing: structured user-facing documentation covering workflows, examples, common patterns, troubleshooting. Currently knowledge is scattered across CLAUDE.md, BACKLOG.md, skill/SKILL.md, and skill/references/.

### Decision needed

- Single `DOCS.md` file vs `docs/` folder with multiple files?
- What should it cover? (Getting started, command reference, workflow recipes, architecture overview?)
- Should it consolidate content currently in skill/references/ (verb-reference.md, exit-codes.md)?

---

## SKILL.md Verification

**Priority:** P1 — skill file may be stale after major hardening session
**Date added:** 2026-03-29

### Problem

`skill/SKILL.md` was updated in the 2026-03-28 hardening session (commit b8d7685) but needs verification that it accurately reflects the current command surface, error handling, and workflow guidance after all the fixes that landed. May have stale references or missing coverage for new capabilities.

### Action

Audit skill/SKILL.md against current `gaal --help`, `gaal <cmd> --help` for all commands, and the actual binary behavior. Fix any gaps.

**Effort:** ~1 hour audit + fixes.

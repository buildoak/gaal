# BACKLOG.md — Future Features & Design Notes

## Subagent First-Class Support

**Priority:** P0 — critical path. The main AX surface shipped; the remaining gap is orphan recovery for pruned subagent files.
**Date:** 2026-03-29 (revised from 2026-03-28 draft)

### Shipped (2026-03-29)

- `src/subagent/` module (`discovery.rs`, `parent_parser.rs`, `engine.rs`) — 237 lines **DONE** [session: 2b0db33c]
- DB indexing: 5,170 subagents, 174 coordinators **DONE** [session: 2b0db33c]
- `gaal ls --include-subagents` **DONE** [session: 2b0db33c]
- `gaal inspect` shows Subagents table for coordinators, `parent_id` for subagents **DONE** [session: 2b0db33c]
- `gaal search` includes subagent content in Tantivy **DONE** [session: 2b0db33c]
- `gaal transcript` DB-backed subagent data (replaces dead `SubagentProgress` pipeline) **DONE** [session: 2b0db33c]
- Facts extraction fix: inline `ContentBlock::ToolUse` now generates `file_read`/`file_write`/`command` facts **DONE** [session: 2b0db33c]
- Transcript DB lookup fix: `.or_else()` on `Some(vec![])` fallback repaired **DONE** [session: 2b0db33c]

## AX Sprint Fixes

### Shipped (2026-03-29)

- JSON error parity (`hint` and `example` fields) **DONE**
- `create-handoff latest` **DONE**
- `find-salt` false success **DONE**
- `--session-type` filter on `ls` **DONE**
- Per-subcommand `--help` (already working via clap) **DONE**
- Read-only DB (already correct, no change needed) **DONE**

### Open Issue (2026-03-29)

- 4,051 orphan subagent files from CC's 30-day cleanup (historical loss, not recoverable without parent JSONL)

### Data Architecture (Verified 2026-03-29)

**Two data sources, complementary roles:**

| Source | Role | What it provides |
|--------|------|-----------------|
| Parent JSONL `toolUseResult` blocks | Fleet index | agentId, totalTokens, totalDurationMs, totalToolUseCount, status, prompt, final output text |
| Subagent JSONL files (`subagents/agent-{agentId}.jsonl`) | Detail store | Full conversation, every tool call, every file read/write, per-turn token usage |

**Dead end - do NOT build on:**
- `SubagentProgress` events - deprecated by CC v2.1.86+. Use only as legacy fallback for pre-v2.1.86 sessions.

**Path from parent to subagent file is deterministic:**
Parent JSONL -> `toolUseResult.agentId` -> `{session_dir}/subagents/agent-{agentId}.jsonl`

### Target AX

**`gaal who read src/render/session_md.rs`**
```
  7d5d03e4  2026-03-28  claude-opus-4-6     -> a59e6762 (Fix Agent rendering in transcripts)
```
Attribution flows through parent to the subagent that did the work.

**`gaal inspect <parent-id>`** - shows Subagents (N) table:
```
  Subagents (34):
  ID        Model          Tokens    Duration  Description
  a59e6762  sonnet-4-6     75K       4m 47s    Fix Agent rendering in transcripts
  a930b582  sonnet-4-6     78K       5m 35s    Investigate context_tokens and caveat title
```
Data comes from parent's `toolUseResult` blocks - no subagent JSONL read needed for this view.

**`gaal inspect <subagent-id>`** - same as any session:
```
  Session: a59e6762 (subagent of 7d5d03e4)
  Model: sonnet-4-6
  Task: "Fix Agent rendering in transcripts"
  Files read: session_md.rs, CLAUDE.md, BACKLOG.md
  Files written: session_md.rs
  Commands: cargo build --release, gaal transcript 7d5d03e4 --stdout
```

**`gaal search`, `gaal recall`** - subagent facts in the same Tantivy index, transparent.

**`gaal ls`** - subagents hidden by default. `--include-subagents` or `--type subagent` to show.

**`gaal transcript`** - renderer pulls subagent summary from DB instead of dead `SubagentProgress` events.

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

1. `src/discovery/claude.rs`: Add `collect_subagent_jsonl_files()` - scan `{session_dir}/subagents/` for `agent-*.jsonl`. Do NOT make `collect_project_jsonl_files()` recursive.
2. `src/discovery/discover.rs`: Add `SubagentInfo` to `DiscoveredSession` (agent_id, parent_session_uuid, description from `.meta.json`).
3. Parse subagent JSONLs through existing Claude parser - format is identical. Override session ID with agentId prefix.
4. Link subagent facts to the subagent session row created in Phase 1.

**Verification gate:** `gaal who read src/render/session_md.rs` shows subagent attribution. `gaal search "cargo build"` returns subagent sessions. `gaal inspect <subagent-id>` shows internal file reads, commands, tool counts.

#### Phase 3: Transcript Renderer Migration (1 day) - SHIPPED 2026-03-29

**Goal:** Replace dead `SubagentProgress` pipeline with DB-backed subagent data.

1. `src/render/session_md.rs`: In Executive Summary, query DB for child sessions instead of building `SubagentDelta` from progress events. DONE
2. Populate summary table from `sessions` table (parent_id query). DONE
3. Populate "Files Touched by Subagents" from subagent facts. DONE
4. Keep `SubagentProgress` parsing as legacy fallback for pre-v2.1.86 sessions where DB hasn't been backfilled. DONE

**Remaining gap:** Task column for v2.1.86+ sessions still prefers first `user_prompt` and not the parent description.

#### Phase 4: Polish + Edge Cases (1 day)

1. Handle orphaned subagents (parent JSONL deleted).
2. Handle zero-turn / empty subagents.
3. `gaal recall` verification - FTS fallback includes subagent content.
4. ID collision testing - agentId 8-char prefixes vs UUID 8-char prefixes across full corpus.
5. `--skip-subagents` flag for `gaal index backfill` (opt-out for speed).

### Performance Impact

- First backfill: ~7,029 files, 1.55 GB, ~30-60 seconds
- DB growth: ~7,029 new session rows + ~150K new facts
- Tantivy rebuild: ~6 seconds (up from <2 seconds)
- Subsequent runs: skip unchanged files via size-based check

### Risks

1. ID collision: agentId 8-char prefix could collide with UUID 8-char prefix. Test against full corpus before shipping.
2. Agent-mux workers: Dispatched via Bash, not Agent tool - no `toolUseResult` in parent JSONL, no subagent JSONL files. These remain invisible until new agent-mux emits proper metadata. Known P2 gap.
3. Pre-v2.1.86 sessions: Have `SubagentProgress` but may not have been backfilled. Phase 1 parsing handles both old and new formats.

---

## AX Polish - Subagent Integration

**Priority:** P1
**Date:** 2026-03-29

- `who` - parent->subagent attribution (P0) **DONE** [session: 2b0db33c]
- `ls` - session_type in output (P1) **DONE** [commit: 80db650]
- `ls` - noise filter + limit bug (P1) **DONE** [commit: 768b923]
- `search` - session_type in results (P1) **DONE** [session: 2b0db33c]
- Transcript frontmatter - subagent shows parent's ID (P1) **DONE** [commit: 80db650]
- `ls` - human mode subagent differentiation (P2) **DONE** [commit: 768b923]

## Transcript Title Caveat Leak

**Priority:** P1 - visible quality bug in transcript titles
**Date added:** 2026-03-29

- **DONE** [commit: 80db650] XML tags are stripped before truncation in `get_first_user_prompt()`, so caveat and voice wrappers no longer leak into transcript titles.

## DOCS.md / Documentation Structure

**Priority:** P1 - gaal now has user-facing documentation in `DOCS.md`
**Date added:** 2026-03-29

- **DONE** [commit: 46712e8] `DOCS.md` now exists, `README.md` points to it, and the old root docs were archived.

## SKILL.md Verification

**Priority:** P1 - skill file may be stale after major hardening session
**Date added:** 2026-03-29

### Problem

`skill/SKILL.md` was updated in the 2026-03-28 hardening session (commit b8d7685) but needs verification that it accurately reflects the current command surface, error handling, and workflow guidance after all the fixes that landed. May have stale references or missing coverage for new capabilities.

### Action

Audit `skill/SKILL.md` against current `gaal --help`, `gaal <cmd> --help` for all commands, and the actual binary behavior. Fix any gaps.

**Effort:** ~1 hour audit + fixes.

## CC Session Cleanup Mitigation

**Priority:** P2
**Date:** 2026-03-29

### Context

CC's `cleanupPeriodDays` was set to 30 by default. On 2026-03-29, verified that 4,051 subagent JSONL files have already been pruned - their parent coordinator sessions survive but the child files are gone. These are unrecoverable without the parent JSONL (`toolUseResult` blocks only exist in the parent, not the orphaned subagent files).

### Actions Taken

- `cleanupPeriodDays` set to **365** on 2026-03-29 - future sessions will not be pruned for a year

### Remaining Work

- **Orphan recovery for existing 4,051 files** (P2): Subagent files that still exist on disk but whose parent JSONL is gone can be indexed independently using the internal `parentUuid` field embedded in each subagent JSONL. Parse `parentUuid` from the subagent file itself to reconstruct the link. This recovers `file_read`/`file_write`/`command` facts and makes the subagent searchable, but fleet-level metadata (`totalTokens`, `totalDurationMs`, `status`, `prompt`) is lost.

## Incremental Parsing (SHA-256 prefix + byte offset resume)

**Source:** tokscale v2.0.0 (`crates/tokscale-core/src/message_cache.rs` + `sessions/codex.rs`)
**Priority:** P3 - would make `gaal index backfill` near-instant for append-only sessions

### Problem

`gaal index backfill` currently has two skip paths:

1. Size-based skip (`last_indexed_offset == file_size`): the session is completely untouched since last run. Skipped, no I/O.
2. Incremental parse (`last_indexed_offset < file_size`): gaal reads from the stored byte offset forward, parses only the new lines, and merges the delta into the existing `SessionRow`. This is already implemented in `parse_session_incremental()`.

What gaal does not do is verify that the bytes before the stored offset have not changed. If a session file is rewritten from the start, gaal will silently parse garbage from mid-file and accumulate incorrect token counts.

### How tokscale does it

tokscale layers file identity checks:

- `file_size` + `mtime`
- `sample_hashes`
- `sha256_prefix` of the first 64KB

It then resumes from `last_byte_offset` only when the prefix still matches.

### Adaptation for gaal

gaal already implements the easy half: byte-offset-based incremental parsing. What it is missing is the prefix trust layer.

| tokscale concept | gaal adaptation |
|---|---|
| `SourceFingerprint` | New struct in `src/parser/common.rs` or `src/parser/fingerprint.rs` |
| `sha256_prefix` | Add `sha2` and store the prefix alongside `last_indexed_offset` |
| `sample_hashes` | Optional, if `mtime` proves unreliable |
| `last_byte_offset` | Already exists as `sessions.last_indexed_offset` |
| Stale regression check | Reject negative merged token deltas |
| Newline boundary check | Assert byte at `offset - 1` is `\n` before reading from `offset` |

**Concrete changes required:**

1. Schema migration: add `sha256_prefix TEXT` to `sessions`.
2. Fingerprint computation: implement `compute_sha256_prefix(path, limit_bytes) -> [u8; 32]`.
3. Fingerprint storage: extend `SessionRow` and persist the prefix.
4. Trust gate: compare current SHA-256 prefix before trusting `last_indexed_offset`.
5. Newline boundary check: fail fast if the resume offset is mid-record.
6. Stale regression guard: if merged totals go backwards, trigger a full reparse.

### What does NOT need to change

- Tantivy rebuild already runs after backfill.
- `--force` already bypasses skip/incremental logic.
- `gaal index reindex <id>` already does a full reparse.

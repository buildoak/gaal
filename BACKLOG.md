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

## Subagent Session Discovery

**Priority:** P0 — coordinator-heavy workflows lose observability over the majority of actual work

### Problem

**What IS captured:** The parent session JSONL records Agent tool_use (dispatch intent: description, model, prompt) and tool_result (final output, agentId, usage stats). Gaal indexes these as facts — so dispatch+result is visible in `gaal search`, `gaal inspect`, and transcripts (once the "Agent" name fix below is applied).

**What IS NOT captured:** The subagent's internal conversation — what files it read, what tools it tried, what decisions it made along the way, intermediate failures. This data lives in separate JSONL files that gaal's discovery never scans.

Claude Code stores subagent sessions in a `subagents/` subdirectory:

```
~/.claude/projects/<project-hash>/<session-id>/subagents/agent-<agent-id>.jsonl
~/.claude/projects/<project-hash>/<session-id>/subagents/agent-<agent-id>.meta.json
```

Gaal's discovery logic (`src/discovery/claude.rs:collect_project_jsonl_files()`) iterates `~/.claude/projects/<project-hash>/*.jsonl` with a single-depth `fs::read_dir` — it does not recurse into subdirectories. The subagent JSONL files are invisible to `gaal who`, `gaal search` (for internal subagent activity), and cannot be inspected individually via `gaal inspect <agent-id>`.

### File counts (as of 2026-03-28, coordinator project only)

Measured in `~/.claude/projects/-Users-otonashi-thinking-pratchett-os-coordinator/`:

- **249 subagent directories** containing at least one subagent file
- **4,001 subagent JSONL files** — full conversation transcripts of individual agent runs
- **2,022 `.meta.json` sidecar files** — lightweight manifests per agent

Across all 29 Claude projects on this machine: **514 subagent directories**, **7,009 subagent JSONL files**, **2,027 `.meta.json` sidecars**.

### What `.meta.json` contains

Each `.meta.json` is a compact JSON object with agent type and description. Example:

```json
{"agentType":"general-purpose","description":"Full spec: gaal query engine"}
```

Fields observed: `agentType` (always `"general-purpose"` so far), `description` (matches the `description` field passed to the `Agent` tool call in the parent session). This is indexable — it gives the task label without parsing the full JSONL.

### Subagent JSONL format (first 5 lines)

Subagent JSONLs are structurally identical to top-level Claude session files. The key differences are:

1. **`isSidechain: true`** — every record carries this flag
2. **`agentId`** — the agent's short hex ID, present on every record
3. **`parentUuid`** — links records to their parent turn in the agent's own conversation
4. The first line is a `type: "user"` record containing the full prompt injected by the parent session
5. Subsequent records alternate `type: "assistant"` / `type: "user"` (tool results) — same schema as top-level sessions

The `sessionId` field on each record is the **parent session's** UUID, not the agent's own ID. The agent's identity is `agentId`.

### How this maps to gaal's existing discovery architecture

`src/discovery/claude.rs` — `collect_project_jsonl_files()` calls `fs::read_dir(&project_path)` once and yields only `*.jsonl` files at that depth. It explicitly skips non-file entries, which means subdirectories (including `subagents/`) are silently skipped.

The parser (`src/parser/claude.rs`) is already capable of parsing subagent JSONLs without modification — the format is compatible. The discovery gap is entirely in `collect_project_jsonl_files()`.

The `SubagentProgress` event kind (populated from `type: "progress"` / `data.type: "agent_progress"` records in the parent session) gives aggregate stats (total tokens, duration, tool count) but not the subagent's internal conversation. Full subagent indexing requires parsing the agent JSONL files directly.

### Fix approach

**Phase 1 — index `.meta.json` only (low effort, high value):** Extend `collect_project_jsonl_files()` to also recurse into `<session-uuid>/subagents/` directories. For each agent, emit a `DiscoveredSession` using the `.meta.json` description as the session summary and the agent JSONL as the path. The `session_id` would be `<parent-short-id>/<agent-id>` to namespace them.

**Phase 2 — full subagent indexing:** Parse agent JSONLs through the existing Claude parser pipeline. Because `isSidechain: true` records use the parent's `sessionId`, gaal needs to derive the agent's identity from `agentId` instead. `parse_claude_head()` would need a fallback: if `sessionId` is absent or matches an already-indexed parent, use `agentId` as the primary key.

**Schema consideration:** A `parent_session_id` column in `sessions` would allow `gaal inspect <parent-id>` to also surface subagent summaries without loading each agent's full index entry.

### Estimated effort

- Phase 1 (meta.json discovery + summary indexing): 1–2 days
- Phase 2 (full subagent JSONL indexing): 3–4 days (parser adaptation + schema migration + `inspect` UX for nested sessions)

---

## Transcript Subagent Rendering Quality

**Priority:** P0 — tool name mismatch causes all Agent dispatches to render as bare `-> Agent` instead of rich subagent blocks

### Problem

Transcripts produced by `gaal transcript` show subagent dispatches as bare `-> Agent` lines with no task description, model, prompt excerpt, result, or token stats. In a coordinator session with 34 Agent calls, every single one renders identically as `-> Agent`.

**What's in the JSONL:** The `Agent` tool_use blocks in the parent session contain full rich data:

```json
{
  "type": "tool_use",
  "name": "Agent",
  "input": {
    "description": "Scout 1: gaal docs/plans sweep",
    "model": "sonnet",
    "prompt": "You are a research scout..."
  }
}
```

The `input` object has `description`, `model`, and `prompt` — everything needed for a rich subagent block.

**What the renderer produces:** `-> Agent` — just the tool name.

### Root Cause

This is **not** caused by today's duplicate ToolUse removal fix (`7e32d62`). That fix was correct — it stopped emitting standalone `ToolUse` events that duplicated what was already inside `AssistantMessage` content blocks.

The actual cause is a **tool name mismatch** in `src/render/session_md.rs`. The `fmt_tool_annotation()` function in the renderer handles `"Task"` as the rich subagent case:

```rust
"Task" => {
    let desc = get_str(&inp, "description")...
    let prompt = get_str(&inp, "prompt")...
    let model = get_str(&inp, "model")...
    Some(ToolAnnotation::Task(TaskInfo { ... }))
}
```

But Claude Code currently names the tool `"Agent"`, not `"Task"`. The `"Agent"` name falls through to the catch-all:

```rust
_ => Some(ToolAnnotation::Simple(format!("-> {name}")))
```

This produces `-> Agent` for every subagent dispatch, discarding all the rich `input` data that is present in the JSONL.

The same mismatch affects `collect_subagents()` in the Executive Summary section — it also checks `if name == "Task"` and finds nothing, leaving the Subagents table empty.

Also confirmed: tool_result content for Agent calls is stored in a persisted-output file (tool-results sidecar) rather than inline in the JSONL. The renderer's `lookup_delta_for_task()` tries to extract `agentId` from tool_result content, but with the name mismatch the Task matching path is never reached.

### Fix Approach

One-line fix in `fmt_tool_annotation()` — add `"Agent"` as an alias for `"Task"`:

```rust
"Task" | "Agent" => {
    let desc = get_str(&inp, "description").unwrap_or("").to_string();
    let prompt = get_str(&inp, "prompt").unwrap_or("").to_string();
    let model = get_str(&inp, "model").unwrap_or("sonnet").to_string();
    let subagent_type = get_str(&inp, "subagent_type").unwrap_or("").to_string();
    Some(ToolAnnotation::Task(TaskInfo {
        description: desc,
        prompt,
        model,
        subagent_type,
        tool_id: tool_id.to_string(),
    }))
}
```

Same fix needed in `collect_subagents()`:

```rust
if name == "Task" || name == "Agent" {
```

These two changes restore full subagent rendering (description, model, prompt, result, delta stats) without reintroducing the duplication bug — the dedup fix is orthogonal and remains correct.

**Secondary issue:** The `agentId` extraction from tool_result content (`lookup_delta_for_task`) depends on inline result content, but Agent tool results are frequently offloaded to persisted-output sidecars. The renderer never reads those sidecars. This means even after the name fix, `total_tokens` / `total_duration_ms` from tool results may not populate. Stats from `SubagentProgress` events (the `type: "progress"` / `agent_progress` path) will still work since those are inline in the parent JSONL.

---

## Subagent First-Class Indexing — Design Spec

**Priority:** P0 — coordinator-heavy workflows have 7,014 subagent JSONL files (1.55 GB) that are completely invisible to search, who, inspect, and recall. The majority of actual work product lives in subagent conversations.

**Date:** 2026-03-28
**Author:** Research phase, grounded in code + data examination

### 1. Current State — Evidence-Based Assessment

#### 1.1 What Exists on Disk

Subagent JSONL files live at:
```
~/.claude/projects/<project-hash>/<session-uuid>/subagents/agent-<agent-id>.jsonl
~/.claude/projects/<project-hash>/<session-uuid>/subagents/agent-<agent-id>.meta.json
```

Corpus stats (measured 2026-03-28):
- **7,014 subagent JSONL files** across all projects
- **514 parent sessions** have subagent directories
- **1,554.7 MB** total subagent JSONL data
- **Average 158.8 KB** per subagent JSONL (100-file sample)
- Top parent sessions have 22+ subagents each

#### 1.2 Subagent JSONL Format (Verified)

Subagent JSONLs are **structurally identical** to top-level Claude sessions. Each record carries:

| Field | Value | Note |
|-------|-------|------|
| `type` | `"user"` / `"assistant"` | Same alternation as parent |
| `sessionId` | Parent's UUID | **NOT the subagent's own ID** |
| `agentId` | `"a158cda3175a067a7"` | The subagent's unique identity |
| `isSidechain` | `true` | Always true for subagents |
| `parentUuid` | UUID or null | Links records within the agent's own conversation |
| `cwd` | Absolute path | Inherited from parent |
| `message.model` | `"claude-sonnet-4-6"` etc. | Present on assistant records |
| `message.usage` | Full usage object | Input/output/cache tokens present |
| `message.content` | Tool_use, text blocks | Full tool calls (Read, Bash, Write, etc.) |

The `.meta.json` sidecar is a compact object:
```json
{"agentType":"general-purpose","description":"Full spec: gaal query engine"}
```

Fields: `agentType` (always `"general-purpose"` observed), `description` (matches the `description` passed to the `Agent` tool call).

#### 1.3 What the Parent JSONL Contains for Agent Calls

In the parent session JSONL:

- **Agent tool_use**: `{type: "tool_use", name: "Agent", input: {description, model, prompt}}` — dispatch intent with full prompt
- **Agent tool_result**: Full subagent output text (content array, ~6KB average). Content is inline for smaller results, offloaded to `tool-results/<hash>.txt` sidecars for larger ones
- **SubagentProgress events** (`type: "progress"`, `data.type: "agent_progress"`): carry `agentId` but `totalTokens`, `totalDurationMs`, `totalToolUseCount` are **always null** in observed data — the fields exist but are never populated by the Claude Code runtime

The existing Claude parser (`src/parser/claude.rs`) already emits `EventKind::SubagentProgress` from progress records. The facts consumer (`src/parser/facts.rs:341-343`) explicitly ignores them.

#### 1.4 Schema Infrastructure Already in Place

The schema (`src/db/schema.sql`) already has:
- `sessions.parent_id TEXT REFERENCES sessions(id)` — column exists, **never populated** by any code path
- `sessions.session_type TEXT CHECK(session_type IN ('standalone', 'coordinator', 'subagent'))` — column exists, always set to `'standalone'`
- `CREATE INDEX idx_sessions_parent ON sessions(parent_id)` — index exists
- `CREATE INDEX idx_sessions_type ON sessions(session_type)` — index exists

The `SessionRow` struct in `queries.rs` has `session_type: String` but no `parent_id` field. The `upsert_session()` SQL does not include `parent_id`.

#### 1.5 Discovery Gap

`src/discovery/claude.rs::collect_project_jsonl_files()` does a single-depth `fs::read_dir()` on each project directory, filtering for `.jsonl` files. It explicitly skips non-file entries. Subagent directories (`<session-uuid>/subagents/`) are directories inside `<session-uuid>/` directories — two levels of nesting that the current code never enters.

### 2. Design

#### 2.1 Discovery — Extending `collect_project_jsonl_files()`

**Approach:** Add a separate function `collect_subagent_jsonl_files()` that scans for the known path pattern. Do NOT make `collect_project_jsonl_files()` recursive — the two-level nesting pattern for subagents is specific and stable. General recursion would pick up `tool-results/` directories and other non-session content.

```rust
/// Discover subagent JSONL files from
/// `~/.claude/projects/<hash>/<session-uuid>/subagents/agent-<id>.jsonl`.
fn collect_subagent_jsonl_files(root: &Path) -> Vec<SubagentFile> {
    let mut files = Vec::new();
    // Level 1: project dirs
    for project_entry in fs::read_dir(root).ok().into_iter().flatten().flatten() {
        if !project_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) { continue; }
        // Level 2: session dirs (UUID-named directories containing subagents/)
        for session_entry in fs::read_dir(project_entry.path()).ok().into_iter().flatten().flatten() {
            if !session_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) { continue; }
            let subagents_dir = session_entry.path().join("subagents");
            if !subagents_dir.is_dir() { continue; }
            let parent_session_id = session_entry.file_name().to_string_lossy().to_string();
            // Level 3: agent-<id>.jsonl files
            for agent_entry in fs::read_dir(&subagents_dir).ok().into_iter().flatten().flatten() {
                let path = agent_entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("agent-") && name.ends_with(".jsonl") {
                    let agent_id = name.trim_start_matches("agent-").trim_end_matches(".jsonl");
                    files.push(SubagentFile {
                        jsonl_path: path,
                        agent_id: agent_id.to_string(),
                        parent_session_uuid: parent_session_id.clone(),
                        meta: load_meta_json(&subagents_dir, agent_id),
                    });
                }
            }
        }
    }
    files
}

struct SubagentFile {
    jsonl_path: PathBuf,
    agent_id: String,
    parent_session_uuid: String,         // Full UUID of parent session
    meta: Option<SubagentMeta>,          // From .meta.json sidecar
}

struct SubagentMeta {
    agent_type: String,                  // "general-purpose"
    description: String,                 // Task description
}
```

**`discover_claude_sessions()` changes:** Call both `collect_project_jsonl_files()` (existing, for parent sessions) and `collect_subagent_jsonl_files()` (new). Emit `DiscoveredSession` from both, with subagent sessions carrying extra metadata.

#### 2.2 Session Identity — The agentId Problem

**Problem:** Subagent JSONLs carry `sessionId` = parent's UUID. Using `sessionId` as the session primary key would collide with the parent. The actual identity is `agentId`.

**Solution:** For subagent sessions, the primary key in `sessions.id` is the `agentId` value (e.g., `a158cda3175a067a7`), truncated to 8 chars for the short ID (consistent with Claude sessions using first-8 of UUID).

**Parent linkage:** The `parent_session_uuid` from the file path maps to the parent's full UUID. To populate `sessions.parent_id`, we need the parent's short ID. Two approaches:

1. **Path-based:** The parent session's file stem IS the full UUID. The short ID is `uuid[..8]`. Compute during discovery.
2. **DB lookup:** After indexing the parent, query `sessions` by jsonl_path matching the parent UUID. More reliable but adds a dependency ordering.

**Recommendation:** Path-based derivation. The parent's short_id is `parent_session_uuid[..8]`. If the parent hasn't been indexed yet, set `parent_id` to `NULL` and backfill on subsequent runs via:
```sql
UPDATE sessions SET parent_id = :parent_short_id
WHERE id = :agent_short_id AND parent_id IS NULL
```

#### 2.3 DiscoveredSession Extension

Add an optional `SubagentInfo` to `DiscoveredSession`:

```rust
pub struct DiscoveredSession {
    pub id: String,
    pub engine: Engine,
    pub path: PathBuf,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub file_size: u64,
    // New:
    pub subagent_info: Option<SubagentInfo>,
}

pub struct SubagentInfo {
    pub agent_id: String,                // Full agent hex ID
    pub parent_session_id: String,       // Short (8-char) parent ID
    pub parent_session_uuid: String,     // Full parent UUID
    pub description: Option<String>,     // From .meta.json
    pub agent_type: Option<String>,      // From .meta.json
}
```

When `subagent_info` is `Some`, the indexer sets:
- `session_type` = `"subagent"`
- `parent_id` = `subagent_info.parent_session_id`

#### 2.4 Schema Changes

**No DDL migration needed.** The schema already has `parent_id` and `session_type` with the correct CHECK constraint (`'standalone', 'coordinator', 'subagent'`). The `idx_sessions_parent` and `idx_sessions_type` indexes already exist.

**`SessionRow` struct change:** Add `parent_id: Option<String>` field. Update `upsert_session()` SQL to include the `parent_id` column in both INSERT and ON CONFLICT clauses. This is the only queries.rs change.

**New fact type:** Add `'subagent_result'` to the `facts.fact_type` CHECK constraint. This captures the subagent's final output from the parent JSONL's Agent tool_result. Requires a one-time ALTER TABLE:
```sql
-- Migration: expand fact_type CHECK constraint to include subagent_result
-- SQLite doesn't support ALTER CHECK, so this is handled by CREATE TABLE IF NOT EXISTS
-- on fresh DBs, and by dropping the constraint check for existing DBs via
-- a new table + copy migration (or just insert raw — SQLite CHECK constraints
-- are not enforced when PRAGMA ignore_check_constraints is set temporarily).
```

**Simpler alternative:** Use existing `task_spawn` for the dispatch and `assistant_reply` for the result. Avoids schema migration entirely. The `subject` field on the assistant_reply fact can carry the agentId to differentiate subagent results from normal assistant replies.

**Recommendation:** Use existing fact types. `task_spawn` already captures Agent/Task dispatches. For the result, create an `assistant_reply` fact with `subject = "agent:<agent_id>"` and `detail = <truncated output>`. No schema migration, no CHECK constraint change.

#### 2.5 Fact Extraction from Subagent JSONLs

**Parser compatibility:** The existing Claude parser (`src/parser/claude.rs::parse_events()`) already handles subagent JSONLs correctly — same record types, same field paths. Verified by examining actual subagent JSONL records: they have `type: "user"/"assistant"`, `message.content`, `message.usage`, `message.model` in the same structure.

**Session ID override:** The parser extracts `sessionId` from the first record as the session ID. For subagents, this is the parent's UUID. The facts consumer already falls back to the file stem when `sessionId` matches a known parent.

**Solution:** In `extract_parsed_session()`, after initial parse, check if the JSONL carries `isSidechain: true` and `agentId`. If so, override `meta.id` with the `agentId`. This requires a small extension to `parse_claude_head()` to also extract `agentId` and `isSidechain`.

Alternatively (simpler): The `DiscoveredSession` already provides the correct `id` from the filename. The `index_discovered_session()` function already overrides the session ID via `session_row.id = target_id.to_string()`. No parser changes needed — the override happens at the indexing layer.

**Recommendation:** No parser changes. The indexer already overrides the ID. The `DiscoveredSession` constructed from the subagent file path already has `id = agent_id[..8]`.

#### 2.6 Fact Extraction from Parent JSONL — Agent tool_result

Currently, the Agent tool_result content in the parent session is stored as a `ToolResult` event but the fact extraction in `facts.rs` only creates an error fact (if `is_error` is true or shell non-zero exit) or backfills exit_code on shell tools. Non-shell tool results are silently dropped.

**Change:** In `facts.rs`, when processing a `ToolResult` whose matching `ToolCallState.tool_name` is `"Agent"` or `"Task"`, create an `assistant_reply` fact:

```rust
// In the ToolResult match arm:
let is_agent_tool = matches!(tool_name, "Agent" | "Task");
if is_agent_tool {
    if let Some(output) = output_text.as_ref() {
        let truncated = truncate(output, 2000);
        facts.push(Fact {
            id: None,
            session_id: String::new(),
            ts: ts_str.clone(),
            turn_number,
            fact_type: FactType::AssistantReply,
            subject: Some(format!("agent_result:{}", tool_use_id)),
            detail: Some(truncated),
            exit_code: None,
            success: None,
        });
    }
}
```

This makes subagent outputs searchable via `gaal search` even without parsing the full subagent JSONL — the parent session's indexed facts include a summary of what each subagent returned.

#### 2.7 SubagentProgress / SubagentCompletion Events

Currently ignored in `facts.rs:341-343`.

**Assessment:** In observed data, `SubagentProgress` events carry `agentId` and `prompt` but `totalTokens`, `totalDurationMs`, and `totalToolUseCount` are **always null**. These events are heartbeat signals, not completion summaries. The actual completion data (tokens, content) comes from the tool_result.

**Recommendation:** Continue ignoring `SubagentProgress` for fact extraction. Instead, use them to populate subagent session metadata:

- On first `SubagentProgress` for a given `agentId`, record the `prompt` as the subagent's task description (useful when `.meta.json` is missing)
- Accumulate the last-seen `totalTokens`/`totalDurationMs` if they become populated in future Claude Code versions

This is a Phase 3 enhancement. Not required for core indexing.

#### 2.8 Deduplication Strategy

**Problem:** The parent JSONL has the dispatch (task_spawn fact) + output (proposed assistant_reply fact). The subagent JSONL has the full internal conversation (potentially 50+ facts: reads, writes, commands, replies). Indexing both creates overlapping but non-identical records.

**Assessment:** This is NOT a deduplication problem — it is a **complementary data** problem.

- **Parent session facts** about Agent calls: task_spawn (what was dispatched), agent_result (what came back). These are the coordinator's perspective.
- **Subagent session facts**: file reads, writes, commands, errors, internal reasoning. These are the worker's perspective.

The data does not overlap. The parent never contains the subagent's internal tool calls. The subagent never contains the coordinator's dispatch metadata.

**One edge case:** The subagent's first `user` record contains the full prompt text, which is also in the parent's Agent `tool_use.input.prompt`. This creates two `user_prompt` facts with overlapping content. Acceptable: one is tagged to the parent session, the other to the subagent session. Different session_ids, different contexts.

**Recommendation:** No deduplication logic needed. Index both independently.

#### 2.9 Query Surface Changes

##### `gaal ls`

**Default behavior:** Exclude subagents. Showing 7,014 additional sessions in the fleet view would overwhelm the output and bury the 2,433 parent sessions.

**Flag:** `--include-subagents` (or `--subagents`) to include them. When included, subagent rows are visually distinguished with a prefix indicator.

**Implementation:** Add `session_type` filter to `list_sessions()` SQL:
```sql
AND (:exclude_subagents = 0 OR s.session_type != 'subagent')
```

Default: `exclude_subagents = 1`. With `--include-subagents`: `exclude_subagents = 0`.

Alternatively, add `session_type` to `ListFilter` (cleaner):
```rust
pub struct ListFilter {
    // ... existing fields ...
    pub session_type: Option<String>,     // None = all, Some("standalone") = exclude subagents
    pub exclude_subagents: bool,          // Simpler: default true
}
```

##### `gaal inspect <id>`

**For subagent IDs:** Works exactly like parent sessions — shows session detail, files, commands, timeline, git ops. The `session_type = "subagent"` and `parent_id` are displayed in the output header.

**For parent IDs:** Add a "Subagents" section at the bottom:
```
Subagents (12):
  a158cda3  sonnet  "Code quality audit"         55 tools  12.4K tokens
  a0726ee8  opus    "Session analytics sweep"     23 tools   8.2K tokens
  ...
```

This queries `SELECT * FROM sessions WHERE parent_id = :parent_id` using the existing `idx_sessions_parent` index.

##### `gaal search`

**No changes needed.** The Tantivy FTS index already indexes all facts regardless of session_type. Once subagent facts are in the DB, they are automatically searchable. Search results will include subagent session IDs — the UI may want to annotate them with a `[subagent]` tag.

##### `gaal who`

**No changes needed.** `query_who()` joins `facts` with `sessions` and filters by fact_type + subject. Subagent file_read/file_write/command facts will appear in results with the subagent's session ID. The session engine and model are already displayed per result.

**Enhancement (optional):** Annotate `who` results with `[subagent of <parent-id>]` for subagent sessions.

##### `gaal recall`

**No changes needed for handoff-based recall.** Subagents are unlikely to have handoff documents.

**FTS fallback recall** will automatically include subagent content once indexed.

##### `gaal transcript`

**For subagent IDs:** Generate a transcript of the subagent's internal conversation. Works out of the box — the JSONL format is identical.

**For parent IDs:** The transcript already renders Agent tool_use blocks (once the `"Task" | "Agent"` name fix from the previous FEATURES.md section is applied). Subagent transcripts are reachable by inspecting the parent and navigating to the subagent ID.

#### 2.10 Performance Impact

**Discovery overhead:** 514 additional directories to scan (session UUID dirs that contain `subagents/`). Each requires one `fs::read_dir()` to check for `subagents/` existence, then one more to list agent files. Total: ~1,028 additional `readdir` syscalls. Negligible on local SSD.

**Indexing overhead for backfill:** 7,014 additional JSONL files, 1.55 GB total. At gaal's current parsing rate (~50 MB/s on Apple Silicon), full backfill adds ~31 seconds. With incremental parsing (already implemented), subsequent runs only parse new/changed files.

**Database growth:** ~7,014 new session rows + estimated ~150K new facts (average 21 facts per subagent session based on tool usage patterns observed). The current DB has ~2,433 sessions and ~200K facts. This approximately triples the fact count but SQLite handles millions of rows without issue.

**Search index impact:** Tantivy rebuild time scales linearly with fact count. Current rebuild is <2 seconds. With 3x facts, expect <6 seconds. Acceptable.

**Recommendation:** No special performance measures needed for Phase 1. If backfill time becomes a concern at scale, add a `--skip-subagents` flag to `gaal index backfill`.

### 3. Implementation Phases

#### Phase 1: Discovery + Basic Indexing (3 days)

**Goal:** Subagent sessions appear in `gaal ls --include-subagents` and `gaal inspect <agent-id>`.

**Changes:**
1. `src/discovery/claude.rs`: Add `collect_subagent_jsonl_files()`, `SubagentFile`, `SubagentMeta` structs. Extend `discover_claude_sessions()` to call it and emit `DiscoveredSession` with `subagent_info`.
2. `src/discovery/discover.rs`: Add `SubagentInfo` to `DiscoveredSession`.
3. `src/db/queries.rs`: Add `parent_id: Option<String>` to `SessionRow`. Update `upsert_session()` to include `parent_id`. Update `row_to_session()` to read it.
4. `src/commands/index.rs`: In `index_discovered_session()`, when `discovered.subagent_info.is_some()`, set `session_type = "subagent"` and `parent_id`. Use agent short_id as session ID.
5. `src/commands/ls.rs`: Add `--include-subagents` flag. Default: filter out `session_type = 'subagent'`. Update `ListFilter` with `exclude_subagents: bool`.
6. `src/db/queries.rs`: Add `exclude_subagents` to `list_sessions()` WHERE clause.

**Verification gate:** After `gaal index backfill`, `gaal ls --include-subagents --limit 10` shows subagent sessions with correct parent_id, model, timestamps, and token counts. `gaal inspect <agent-short-id>` shows the subagent's internal facts (file reads, commands, etc.).

#### Phase 2: Full Fact Extraction + Search/Who (2 days)

**Goal:** Subagent internal content is searchable. Agent tool_results in parent sessions become indexed facts.

**Changes:**
1. `src/parser/facts.rs`: In the `ToolResult` match arm, detect Agent/Task tool results and create `assistant_reply` facts with `subject = "agent_result:<tool_use_id>"`.
2. Verify `gaal search` returns results from subagent content.
3. Verify `gaal who read <file>` returns subagent sessions that read the file.
4. Optional: annotate search/who results with `[subagent]` indicator.

**Verification gate:** `gaal search "cargo build"` returns subagent sessions that ran cargo builds. `gaal who wrote /src/main.rs` includes subagent sessions. Agent tool_result content appears in parent session search results.

#### Phase 3: Inspect Subagent Summary + Transcript (2 days)

**Goal:** `gaal inspect <parent-id>` shows a subagent summary table. Transcripts render correctly for subagents.

**Changes:**
1. `src/commands/inspect.rs`: When inspecting a parent session, query child sessions via `SELECT * FROM sessions WHERE parent_id = :id` and render a summary table.
2. Apply the `"Task" | "Agent"` name fix from the Transcript Rendering section (if not already done).
3. Verify `gaal transcript <agent-short-id>` generates a valid transcript.
4. Optional: Mark parent sessions as `session_type = "coordinator"` when they have indexed subagents.

**Verification gate:** `gaal inspect 7d5d03e4` shows "Subagents (35)" with agent IDs, models, descriptions, token counts. `gaal transcript <any-agent-id>` produces a readable markdown transcript.

#### Phase 4: Recall + Polish (1 day)

**Goal:** Subagent content participates in recall. Edge cases handled.

**Changes:**
1. Verify `gaal recall <topic>` surfaces subagent content via FTS fallback.
2. Handle orphaned subagents (subagent dir exists but parent session JSONL deleted). Set `parent_id = NULL`, still index.
3. Handle zero-turn subagents (empty or malformed agent JSONL). Skip, same as parent zero-turn logic.
4. Add `--skip-subagents` flag to `gaal index backfill` for users who want faster backfill.

**Verification gate:** `gaal recall "gaal code quality"` returns the subagent that did the code audit. Orphaned subagents are indexed without error.

### 4. Effort Estimate

| Phase | Work | Duration |
|-------|------|----------|
| Phase 1 | Discovery + basic indexing | 3 days |
| Phase 2 | Fact extraction + search/who | 2 days |
| Phase 3 | Inspect summary + transcript | 2 days |
| Phase 4 | Recall + polish | 1 day |
| **Total** | | **8 days** |

### 5. Risks and Mitigations

**Risk: Schema version conflict.** Adding `parent_id` to `SessionRow` and `upsert_session()` requires coordinating with any other in-flight schema changes. The column already exists in the DDL so no ALTER TABLE is needed — just wire it into the Rust structs.

**Risk: ID collision.** Agent IDs are 17-char hex strings (e.g., `a158cda3175a067a7`). Truncating to 8 chars (`a158cda3`) could collide with Claude session UUID prefixes (also 8 chars). Mitigation: prefix subagent short IDs with `a` (they already start with `a` — the `agent-` prefix strips to `a<hex>`). The probability of collision with UUID v4 first-8 chars is negligible but should be tested against the full corpus.

**Risk: Incremental parsing for subagents.** Subagent JSONLs are typically written once (agent runs, completes, file is finalized). Incremental parsing is less important than for long-running parent sessions. Full parse on each backfill run is acceptable for Phase 1.

**Risk: 7,014 files in a single backfill.** First backfill will take ~30-60 seconds extra. Subsequent runs will skip unchanged files (size-based skip). Acceptable.

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

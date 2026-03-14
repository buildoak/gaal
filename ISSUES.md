# ISSUES.md — Known Bugs & Improvements

Tracked: 2026-03-09

---

## I1: `gaal ls` noise — trivial sessions dominate output [FIXED 2026-03-09]

**Severity:** High (UX + token waste)
**Command:** `gaal ls -H --since 2026-03-01`
**File:** `src/commands/ls.rs:270-296` (build_filter), `ls.rs:150-166`

**Problem:** 31 out of 50 results are codex-spark micro-sessions (0-5s duration, 0-2 tool calls). No noise filtering exists — `build_filter` passes all sessions through. Default `--limit 50` fills with junk before reaching substantive sessions.

**Expected:** Default quality gate — `duration > 60s OR tool_calls > 2`. Trivial sessions should be excluded unless `--all` flag is passed.

**Sub-issue:** `child_count` is always 0 across all sessions. Parent-child linking appears non-functional — no sessions have `parent_id` set. The `--children` flag has no practical effect.

**Fix:** Add `--min-duration` and `--min-tools` flags with sensible defaults (60s, 2). Add `--all` to bypass. Investigate why parent-child linking produces no results (possibly `index link-parents` not running or linker logic broken).

---

## I2: `gaal show -H` missing fields vs JSON [FIXED 2026-03-09] [REMOVED v0.1.0]

NOTE: `gaal show` command removed in v0.1.0 - functionality merged into `gaal inspect`.

**Severity:** Medium
**Command:** `gaal show abb9d05a -H` vs `gaal show abb9d05a`
**File:** `src/commands/show.rs:854-972` (print_human)

**Problem:** JSON output has 20 fields. Human output shows ~12. Missing from `-H`:
- `commands` (requires `--commands` flag)
- `files` (requires `--files` flag)
- `errors` (requires `--errors` flag)
- `git_ops` (requires `--git` flag)
- `exit_signal` — never rendered in human mode
- `last_event_at` — never rendered
- `children` (as list) — never rendered

JSON includes all by default (lines 309-313: `include_files = !any_fact_filter`). Human mode gates on explicit flags.

**Expected:** `-H` should show the same richness as JSON by default, formatted as a readable table/sections. The flag-gating is counterintuitive — users expect `-H` to be "same data, human-readable format."

**Fix:** Make human output include commands, files, errors, git_ops by default. Add `exit_signal`, `last_event_at`, `children` to human renderer.

---

## I3: `gaal who --limit` appears ignored [FIXED 2026-03-09]

**Severity:** Low (not a bug, but confusing)
**Command:** `gaal who wrote README.md -H --limit 40` → 5 results
**File:** `src/commands/who.rs:19` (default --since 7d), `who.rs:114-117`

**Problem:** `--limit` works correctly as an upper bound. The actual constraint is `--since` defaulting to `7d` — only 7 days of facts are queried. Users expect `--limit 40` to mean "give me 40 results" but get fewer because the time window is narrow.

Additional: SQL query in `queries.rs:641-694` does NOT filter by `fact_type` — fetches all types, then `who.rs` post-filters via `matches_verb`. This wastes DB rows when the verb is specific.

**Fix:** Either extend default `--since` to 30d, or auto-expand the time window when result count < limit. Push `fact_type` filtering into SQL for efficiency.

---

## I4: `gaal recall` shallow — searches handoffs only [FIXED 2026-03-09]

**Severity:** High (limits usefulness of semantic memory)
**Command:** `gaal recall "tickets" --limit 10` → 1 result
**File:** `src/commands/recall.rs:143-213` (load_all_handoffs)

**Problem:** `recall` searches ONLY the `handoffs` table — headline, projects, keywords fields. Uses custom TF-IDF with 14-day recency decay. Never touches facts or conversation text. If handoff metadata doesn't contain the query terms, the session is invisible.

`gaal search "tickets" --limit 20` returns 6+ results because it indexes ALL facts via Tantivy BM25.

**The gap:** A session where tickets were extensively discussed and built will be missed by `recall` if the handoff LLM didn't include "tickets" in the keywords. `recall` depends entirely on handoff extraction quality.

**Expected:** `recall` should be the primary semantic memory tool. It should search both handoff metadata AND high-signal facts (user prompts, file writes) with handoff matches weighted higher.

**Fix options:**
1. Expand `recall` to also search Tantivy facts index, merge-rank results
2. Improve handoff extraction to capture more keywords from session content
3. Add a `--deep` flag that falls through to fact-level search when handoff results are sparse

---

## I5: `gaal active -H` outputs JSON instead of table [FIXED 2026-03-09] [REMOVED v0.1.0]

NOTE: `gaal active` command removed in v0.1.0 - process monitoring discontinued.

**Severity:** Medium
**Command:** `gaal active -H`
**File:** `src/commands/active.rs:110`, `active.rs:795-804`

**Problem:** The `-H` flag merely toggles `serde_json::to_string_pretty` vs compact JSON. There is no human table formatter for `ActiveOutput`. Unlike `ls` and `who` which have proper `print_table` implementations, `active` has only a local `print_json` function.

**Expected:** `gaal active -H` should output a table like `ls -H` does — columns for ID, engine, status, duration, stuck reason, context%.

**Fix:** Implement `print_table` for active output, matching the pattern used in `ls.rs`.

---

## I7: `gaal handoff <id>` — silent freeze during processing [FIXED 2026-03-09]

**Severity:** Medium (UX)
**Command:** `gaal handoff 2c74e8c0`
**File:** `src/commands/handoff.rs`

**Problem:** Running `gaal handoff <session-id>` freezes with no output while the LLM processes the handoff. No indication that work is happening — looks like a hang. Only shows output once fully complete. For large sessions this can take 30-60+ seconds of silence.

**Expected:** Immediate stderr feedback showing:
1. "Generating handoff for session 2c74e8c0..." (confirm the command was received)
2. Processing details: model/engine being used, prompt mode, session size (turns/tokens)
3. Progress indicator or at minimum a "this may take a moment" note
4. On completion: path to generated handoff file

**Fix:** Add `eprintln!` progress messages in the handoff processing pipeline before the LLM call. Show: session ID, engine, model, prompt source (default/custom), session stats (turns, tokens, duration). Consider a simple spinner or elapsed-time counter for long waits.

---

## I6: `gaal active` stuck detection — false positives + config inconsistency [FIXED 2026-03-09] [REMOVED v0.1.0]

NOTE: `gaal active` command and stuck detection removed in v0.1.0.

**Severity:** Medium
**Command:** `gaal active -H`
**File:** `src/model/status.rs:86-111`, `src/commands/active.rs:190-211`

**Problem — False positives:**
Stuck criteria (any triggers "stuck"):
1. `silence_secs >= 300` AND NOT `permission_blocked` — 5min silence
2. `loop_detected` — last 6 actions have ≤2 unique signatures
3. `context_pct >= 95%`
4. `permission_blocked` — pending tool_use without tool_result

Issue: A session doing a legitimate long computation (large build, heavy inference) produces no JSONL events for 5+ minutes → marked stuck. No concept of "expected long duration" or per-engine thresholds. Codex sessions legitimately run 10-20min builds.

**Problem — Config inconsistency:**
`ls.rs:155` reads `load_config().stuck.silence_secs` from config file. `active.rs:210` hardcodes `STUCK_SILENCE_SECS = 300`. Two different code paths, two different sources of truth.

**Fix:**
1. Read `stuck.silence_secs` from config in `active.rs` (match `ls.rs` behavior)
2. Add per-engine silence thresholds (codex: 600s, claude: 300s)
3. Consider: if last event was a `Bash` tool_use with no result yet, extend silence tolerance (build in progress)

---

## I8: "database locked" under parallel load [FIXED 2026-03-09]

**Severity:** Medium (reliability under concurrent access)
**Command:** `gaal ls` from Codex subagent while cron backfill is running
**File:** `src/db/schema.rs` (init_db), `src/db/queries.rs`

**Problem:** Every gaal invocation — even read-only commands like `ls`, `show`, `search` — opens the DB in read-write mode and runs DDL (ALTER TABLE, CREATE TABLE/INDEX IF NOT EXISTS) during `init_db()`. When the cron backfill is writing (upsert + 71k facts + Tantivy rebuild) and a parallel read command tries DDL → lock contention → 5s busy_timeout exceeded → "database locked" error.

No read-only connection path exists. `gaal ls` takes the same locks as `gaal index`.

**Fix:**
1. Create `open_db_readonly()` — skip DDL, use SQLITE_OPEN_READ_ONLY for read commands
2. Gate schema migration behind version check (don't ALTER TABLE every open)
3. Increase busy_timeout to 30s for write commands
4. Wrap per-session indexing in single transaction (reduce lock churn)

---

## I9: `gaal active` can't find API-spawned Codex sessions [FIXED 2026-03-09] [REMOVED v0.1.0]

NOTE: `gaal active` command removed in v0.1.0 - process monitoring discontinued.

**Severity:** Low-Medium
**Command:** `gaal active` missing running Codex subagent
**File:** `src/discovery/active.rs`

**Problem:** `gaal active` uses `pgrep -x codex` for live process discovery. API-spawned Codex sessions (via agent-mux) have no live process — they exist only as JSONL files in `~/.codex/sessions/` with no PID to discover.

**Fix:**
1. Add mtime-based detection — check `~/.codex/sessions/` for recently-modified JSONL files (mtime < 5min) without matching PID
2. Add `codex-cli`, `codex-rs` to pgrep targets
3. Include API-active sessions in output with distinct discovery source indicator

---

## I10: `gaal handoff` extracts wrong session boundary — child instead of parent [FIXED 2026-03-09]

**Severity:** High (handoff quality — core purpose of gaal)
**Command:** `gaal handoff` (auto-detect mode from within a session)
**File:** `src/commands/handoff.rs` (session resolution logic)

**Problem:** When running `gaal handoff` from within a session that has parent/child relationships (e.g., a main solver session that spawned an audit subagent), gaal auto-detects and extracts the **child session** instead of the parent/main session. The resulting handoff is:
- Wrong scope — covers the child audit task, not the main session's work
- Misleading — useless for resuming the actual project
- Missing execution state — no mention of running processes, campaign status, or the user-facing outcome
- Open threads point to child-session concerns, not the live objective

**Observed behavior (from Codex session):**
- User ran `gaal handoff` expecting a handoff for their main solver/campaign session
- gaal auto-detected the child audit session (`75b2402e`) instead of the parent (`019cd1a0...`)
- Handoff graded 7/10 for the child task scope, **2/10** for the actual session the user wanted

**Expected:** When `gaal handoff` auto-detects, it should:
1. Prefer the parent/root session over child sessions
2. If called from within a child session, warn and offer to extract the parent instead
3. Consider session duration/substance — the long-running parent with more turns/tools is likely the one the user wants

**Root cause hypothesis:** The auto-detection resolves the JSONL file closest to the current process/CWD, which may be the child session's JSONL rather than the parent's. The session resolution logic doesn't account for parent/child hierarchy when choosing which session to extract.

**Fix directions:**
1. In auto-detect mode, check if the resolved session has a `parent_id` — if so, offer/default to the parent
2. When multiple candidate sessions exist for the same CWD, prefer the one with higher substance (more turns, longer duration, more tool calls)
3. Add `--prefer-parent` flag (or make it default) and `--this-session` to explicitly extract the current child

---

## I11: Parent-child session linking is nearly dead — linker rarely fires [FIXED 2026-03-09]

**Severity:** High (foundational — many features depend on parent/child relationships)
**Evidence:** Only 1 out of 2,433 sessions has `parent_id` set in the DB
**File:** `src/linker.rs`, `src/commands/index.rs` (link-parents command)

**Problem:** The `linker.rs` module and `gaal index link-parents` command exist but almost never successfully link parent/child sessions. This means:
- `gaal ls --children` is useless (child_count always 0)
- I10 fix had to use PID-tree heuristic instead of DB parent_id
- `gaal show` children field is always empty
- Session hierarchy is invisible to all commands

**Context:** We use agent-mux extensively to spawn Codex/Claude/OpenCode workers from coordinator sessions. These are real parent/child relationships that should be tracked. The linker needs to understand agent-mux dispatch patterns to correctly link spawned worker sessions back to their parent coordinator session.

**Investigation needed:**
- Why does the current linker fail? What signal does it look for and why doesn't it find it?
- How does agent-mux leave traces in the parent session's JSONL? (Bash tool_use with agent-mux command? Task tool_use?)
- How can we reliably match a parent session's agent-mux dispatch to the child session it spawned?
- What about Claude Code's native Agent tool — does it leave linkable traces?

**Sub-issue (found during verification):** `resolve_child_session_id` uses first-8 hex truncation to find Codex children, but gaal stores Codex IDs as last-8 hex (`truncate_codex_id`). Forward Codex linking silently fails. Fix: add last-8 resolution fallback.

---

## I12: `gaal search` query parser chokes on parentheses [FIXED 2026-03-09]

**Severity:** Medium (search unusable for queries containing special characters)
**Command:** `gaal search "sqrt(36)"` → `{"error":"parse error: invalid search query: Syntax Error: sqrt(36)","exit_code":11,"ok":false}`
**Workaround:** `gaal search "sqrt36"` works (strip parens manually)

**Problem:** Tantivy's query parser treats `(` and `)` as grouping operators. Raw parentheses in search queries cause a syntax error. Users should be able to search for literal text containing parens without escaping.

**Expected:** `gaal search "sqrt(36)"` should either:
1. Auto-escape special characters in the query string before passing to Tantivy
2. Or use Tantivy's `QueryParser::parse_query` with a lenient mode / raw term syntax

**Fix:** In `src/commands/search.rs`, before passing the query to Tantivy's parser, escape or strip Tantivy special characters: `(`, `)`, `[`, `]`, `{`, `}`, `^`, `~`, `:`, `\`, `/`. Alternatively, wrap the entire query in quotes for Tantivy phrase matching.

---

## I13: `gaal search` / `gaal recall` — transient "unable to open database file" under Codex sandbox [FIXED 2026-03-09]

**Severity:** Medium (intermittent, environment-specific)
**Command:** `gaal recall "..."` and `gaal search "..."` from inside Codex subagent
**Context:** A Codex scout session reported both `gaal recall` and `gaal search` failing with "unable to open database file". Later the same commands worked fine from a different context.

**Problem:** Likely caused by Codex sandbox filesystem restrictions. When Codex runs with `--sandbox workspace-write`, access to `~/.gaal/index.db` may be blocked depending on sandbox configuration. The I8 fix (read-only connections) may help, but sandbox path allowlisting may also be needed.

**Expected:** gaal commands should work reliably from within Codex sandbox sessions. If the DB is inaccessible, the error message should be clear: "Cannot access ~/.gaal/index.db — ensure the path is allowed in your sandbox configuration."

**Fix:**
1. Improve error message for DB open failures — include the attempted path
2. Document required sandbox flags for gaal access: `--sandbox workspace-write` may need `--allow-read ~/.gaal/`
3. Consider a `GAAL_DB_PATH` env var override for sandboxed environments

---

## I14: `gaal handoff` hallucinates model/engine for Claude sessions [FIXED 2026-03-10]

**Severity:** Medium (handoff metadata integrity)
**Evidence:** `~/.gaal/data/claude/handoffs/2026/03/09/9c79449b.md` — session is stored under `claude/handoffs/` but frontmatter says `model: GPT-5.3 Codex`, `engine: codex`
**File:** `src/commands/handoff.rs` (handoff extraction prompt / metadata generation)

**Problem:** The LLM performing handoff extraction has no ground-truth signal about which engine produced the session. It guesses from session content — and guesses wrong. Session `9c79449b` is a Claude Code session (lives in `~/.gaal/data/claude/`) but the extracted handoff claims `model: GPT-5.3 Codex` and `engine: codex`.

**Root cause:** The handoff extraction prompt doesn't inject the known engine/source as a constraint. Gaal already knows the engine from the session's storage path and JSONL metadata — this should be passed to the LLM as a hard fact, not left for it to infer.

**Expected:** `model` and `engine` fields in handoff frontmatter should be derived from session metadata (storage path, JSONL headers), not LLM inference. The LLM should fill in headline, projects, keywords, substance — not factual metadata it can get wrong.

**Fix:**
1. Extract engine from session source path (`claude/` → claude, `codex/` → codex) or JSONL metadata
2. Pass engine as a fixed field in the handoff extraction prompt: "Engine: claude. Do not override."
3. Post-validate: if extracted `engine` contradicts the known source, overwrite with ground truth

---

## I15: `gaal handoff` captures planning phase only — misses execution work [FIXED 2026-03-10]

**Severity:** High (handoff quality — session continuity broken)
**Session ID:** cd572b60
**File:** `src/commands/handoff.rs` (handoff extraction / LLM prompt scope)

**Problem:** The generated handoff for session `cd572b60` captured only the planning phase of the session. Significant execution work that followed was entirely absent from the handoff:

- 8-phase GSD build of `wet` (Go binary) — fully built, tested, deployed
- 3 rounds of bug fixes: stderr pollution, HTTP/2 fallback, path doubling, stale stats
- Competitive research across 15 tools
- 6 serendipity seeds filed

A future session reconnecting via this handoff would believe `wet` was still in planning, know nothing of the bugs discovered and fixed, and have no record of the competitive landscape or serendipity work. Session continuity is broken.

**Likely cause:** Two hypotheses, both plausible:
1. **JSONL flush lag** — the session's JSONL file may not have fully flushed by the time `gaal handoff` ran. The LLM only saw the first N turns (planning), not the tail (execution). Large sessions can accumulate writes that lag behind.
2. **Prompt scope too narrow** — the handoff extraction prompt may truncate or summarize early turns aggressively, losing the execution tail in long sessions. Planning phases generate verbose back-and-forth which may dominate token budget, crowding out the execution events that follow.

**Impact:** The handoff is a plausible-looking but deeply incomplete record. It will actively mislead — a reconnecting session would plan work already done, skip bugs already fixed, and miss artifacts already created.

**What was missed:**
- `wet` (Go binary): all 8 build phases completed and verified
- Bug fixes: stderr pollution (output piping), HTTP/2 fallback disabled, path doubling in URL construction, stale stats not refreshing
- Competitive research: 15 tools surveyed, analysis filed
- 6 serendipity seeds written to `centerpiece/serendipity/`

**Suggested fix:**
1. **Scan for file artifacts created during the session** — `gaal show <id> --files` reveals what was written/modified. Handoff extraction should be aware of these artifacts and explicitly summarize what was built, not just what was discussed.
2. **Anchor the handoff to session tail, not session head** — for long sessions, the most recent 20-30% of turns (execution, outcomes, cleanup) carry the highest continuity value. Weight recency in the extraction prompt.
3. **Flush check before extraction** — before running the LLM, verify the JSONL mtime is within 30s of `last_event_at` from the DB. If not, warn: "JSONL may be stale — handoff may be incomplete."
4. **Artifact cross-check** — after LLM extraction, compare the list of files written during the session (from facts DB) against files mentioned in the handoff. If significant files are unmentioned, flag or re-extract with an explicit artifact list injected into the prompt.

---

## I16: `gaal show` returns "not found" for current running session [FIXED 2026-03-10] [REMOVED v0.1.0]

NOTE: `gaal show` command removed in v0.1.0 - functionality merged into `gaal inspect`.

**Severity:** High (blocks handoff of active sessions)
**Session ID:** `019cd256-c7f9-72f0-a2fe-924fe3e8c603`
**File:** `src/commands/show.rs`, `src/commands/handoff.rs` (fallback indexing path)

**Problem:** `gaal handoff` auto-detected the current session via `CODEX_THREAD_ID`, but `gaal show` returned "not found" because the session hadn't been indexed yet. Gaal then attempted on-the-fly indexing as a fallback and crashed with:

```
cannot start a transaction within a transaction
```

**Root cause:** The fallback "index-on-demand" code path opens a transaction for the session upsert, but the caller is already inside a transaction (likely from the handoff pipeline). SQLite doesn't support nested transactions without savepoints.

**Impact:** No handoff was generated for the active session. The user had to fall back to a manual handoff. This is a common scenario — `gaal handoff` from within a running session will frequently hit un-indexed sessions.

**Expected:** `gaal handoff` should gracefully handle un-indexed sessions:
1. Detect the session is not in the DB
2. Index it on-the-fly (outside any existing transaction)
3. Proceed with handoff extraction

**Fix:**
1. Ensure on-the-fly indexing runs in its own connection or commits the outer transaction before starting the index transaction
2. Use `SAVEPOINT` / `RELEASE` for nested transaction support if the caller genuinely needs to stay in a transaction
3. Add a pre-check: if session not found, index first, then proceed — as two sequential operations, not nested

---

## I17: Backfill crashes with "nested transaction" on sessions with subagents [FIXED 2026-03-10]

**Severity:** High (blocks indexing of coordinator sessions)
**Evidence:** `gaal index backfill` crashes on Claude sessions that spawned subagents
**Session ID:** `81e7afb3` (and all recent coordinator sessions)
**File:** `src/commands/index.rs`, `src/db/queries.rs`

**Problem:** `gaal index backfill` crashes with a "nested transaction" SQLite error when processing Claude sessions that have subagent tool calls (Agent tool_use events). The worker had to:
1. Clear stale Tantivy locks manually
2. Manually insert the session record to make handoff work

**Root cause hypothesis:** The indexing pipeline likely opens a transaction per-session, then encounters an Agent tool_use event that triggers child session resolution or linking — which opens its own transaction. SQLite rejects the nested transaction.

**Impact:** All recent coordinator sessions (which heavily use subagents) cannot be indexed via normal backfill. This means:
- `gaal ls` won't show them
- `gaal search` can't find their content
- `gaal handoff` can't extract them (falls back to I16's broken path)
- Session observability is blind to the most important sessions

**Expected:** Backfill should handle sessions with subagent events without crashing. Subagent linking should happen after the main session transaction commits, not inside it.

**Fix:**
1. Separate session indexing (facts extraction + upsert) from parent-child linking into two phases
2. Phase 1: index all sessions in individual transactions (no linking)
3. Phase 2: run `link-parents` as a separate pass after all sessions are indexed
4. Ensure Tantivy index writer is not held across transaction boundaries
5. Add transaction-safety tests for sessions with 5+ subagent tool calls

---

## I18: `gaal ls` shows all sessions as "completed" [FIXED 2026-03-10]

**Severity:** Medium (status field is meaningless)
**Command:** `gaal ls -H`

**Problem:** Every session in `gaal ls` output shows `status: completed` regardless of actual state. Running sessions, errored sessions, interrupted sessions — all show as completed. The status field provides zero signal.

**Expected:** Status should reflect reality:
- `running` — JSONL still being written (mtime recent, no `stop_reason`)
- `completed` — has `stop_reason`, session ended cleanly
- `errored` — ended with error
- `interrupted` — killed mid-stream (truncated JSONL, `stop_reason: None`)

**Investigation needed:** How is `status` determined during indexing? Is the parser extracting `stop_reason` from JSONL? Is the status field being set at all, or defaulting to "completed"?

---

## I19: `gaal active` false positive stuck detection [FIXED 2026-03-10] [REMOVED v0.1.0]

NOTE: `gaal active` command and all stuck detection removed in v0.1.0.

**Severity:** High (alerts on healthy sessions)
**Command:** `gaal active -H`

**Problem:** `gaal active` marks sessions as "stuck" when none are actually stuck. The stuck detection heuristics are too aggressive — legitimate long-running operations (LLM inference, large builds, agent-mux dispatch) trigger false positives.

**Related:** I6 (fixed the config inconsistency but not the fundamental heuristic problem).

**Expected:** Stuck detection should account for:
- Engine-specific silence thresholds (Codex builds can legitimately run 10-20min)
- Pending Bash tool_use without result (build/compilation in progress)
- Agent tool_use dispatches (waiting for subagent completion)
- Context percentage alone shouldn't mean "stuck" — high context is normal for long sessions

---

## I20: `gaal active` lacks session summary — unclear what each session is doing [FIXED 2026-03-10] [REMOVED v0.1.0]

NOTE: `gaal active` command removed in v0.1.0.

**Severity:** Medium (UX — table is just IDs and durations, no context)
**Command:** `gaal active -H`

**Problem:** The active sessions table shows ID, engine, status, duration, stuck reason, context% — but no indication of what each session is actually doing. A coordinator needs to know: "session X is working on ticket B3" or "session Y is running gaal backfill". Without a summary line, the active view requires `gaal show <id>` for each session to understand the fleet.

**Expected:** Each active session should show a one-line summary derived from:
- The session's headline (if indexed)
- The last user prompt (most recent intent)
- The last tool_use action (what it's currently doing)
- CWD (which project directory)

---

## I21: `gaal active` needs first-principles rethink — mixed session types, subagent noise [FIXED 2026-03-10] [REMOVED v0.1.0]

NOTE: `gaal active` command removed entirely in v0.1.0 instead of being fixed.

**Severity:** High (architectural — active view is unusable for fleet management)

**Problem:** `gaal active` is a flat list mixing:
- Main Claude Code coordinator sessions
- Codex workers spawned by agent-mux
- Claude subagents spawned by Agent tool
- TG bot sessions
- Background cron/daemon sessions

This flat list is noise. A coordinator session spawning 5 Codex workers shows as 6 equal entries. The user wants to see: "1 coordinator with 5 workers" — a tree, not a list.

**Sub-issue:** `gaal active` does NOT detect TG bot sessions. These run as persistent processes but use different session discovery patterns.

**Rethink needed:**
1. Group active sessions by hierarchy (coordinator → children)
2. Collapse subagents under their parent by default
3. Add TG bot session discovery (process name, JSONL path pattern)
4. Add `--flat` flag to see the raw list when needed
5. Default view should show only top-level sessions with child count

---

## I22: Active session detection logic — overfitted vs generalizable [CLOSED 2026-03-11]

**Severity:** Medium (architecture)

**Problem:** The current active session detection relies on:
- `pgrep -x claude` / `pgrep -x codex` for process discovery
- tmux pane scanning for session-to-terminal mapping
- JSONL mtime for API-spawned sessions

This works for our specific setup (tmux sessions with named panes) but:
- TG bot sessions are invisible
- Non-tmux users get no terminal mapping
- Different process names (codex-cli, codex-rs) may be missed

**Expected:** Detection should be layered:
1. **Universal:** PID-based process discovery (works everywhere)
2. **Universal:** JSONL mtime-based discovery (catches API sessions)
3. **Optional:** tmux integration (enrichment, not requirement)
4. **Extensible:** Plugin/config for custom session sources (TG bot, daemon processes)

---

## I23: `gaal show` with no parameters shows random session [REMOVED v0.1.0]

NOTE: `gaal show` command removed in v0.1.0.

**Severity:** Low-Medium (confusing UX)
**Command:** `gaal show` (no args)

**Problem:** Running `gaal show` without specifying a session ID shows what appears to be a random session instead of returning an error or showing the most recent session.

**Expected:** Either:
1. Show the most recent session (like `gaal show latest`)
2. Return an error: "session ID required" with usage hint
3. Show the current session if running inside one (auto-detect via PID)

Option 3 is ideal for the common case — a worker wanting to inspect its own session.

---

## I24: `gaal show <id>` not-found for sessions listed in `gaal active` [FIXED 2026-03-10] [REMOVED v0.1.0]

NOTE: Both `gaal show` and `gaal active` commands removed in v0.1.0.

**Severity:** High (active sessions can't be inspected)
**Command:** `gaal show -H f6000264` → `{"error":"not found: f6000264","exit_code":3,"ok":false}`

**Problem:** A session appears in `gaal active` output but `gaal show` returns "not found". This happens because `active` discovers sessions via live process/JSONL detection, but `show` queries the SQLite index — and the session hasn't been indexed yet.

**Related:** I16 (same not-found → fallback index problem, but I16 was about handoff, this is about show).

**Expected:** `gaal show` should:
1. Try the index first (fast path)
2. If not found, check if the session's JSONL exists on disk
3. If JSONL exists, either index on-the-fly or parse directly for a live view
4. Only return "not found" if no JSONL file exists at all

---

## I25: Default gaal outputs are too token-heavy for agent consumption

**Severity:** High (token waste — agents accidentally consuming thousands of tokens)

**Problem:** gaal's default JSON output mode dumps everything — full fact lists, all files, all commands, all errors. When an agent runs `gaal ls` or `gaal show`, it gets a massive JSON blob that blows its context budget. A simple `gaal ls --limit 10` can return 5000+ tokens.

**Expected:** Default outputs should be brief:
- `gaal ls` default: ID, engine, status, duration, headline (one line per session)
- `gaal show` default: summary view (headline, duration, engine, files written count, commands count) — NOT full lists
- `gaal search` default: ID, score, matched snippet (one line per result)
- Detailed output via `--verbose` or `--full` flag
- Human mode (`-H`) should also be concise by default

**Principle:** Every gaal command's default output should fit in ~500 tokens for typical results. Agents should never accidentally blow their context on a gaal call.

---

## I26: `gaal inspect -H` outputs JSON instead of human-readable format

**Severity:** Medium (same pattern as I5)
**Command:** `gaal inspect -H <id>`

**Problem:** Same issue as I5 (which was about `gaal active -H`). The `-H` flag for `inspect` still outputs JSON instead of a formatted human-readable table/sections.

**Expected:** `inspect -H` should show a readable health dashboard: context %, velocity, stuck signals, action summary — formatted as a table or sections, not JSON.

---

## I27: Automatic handoff generation — "magical" zero-friction handoffs

**Severity:** Feature request (deferred — too many bugs to fix first)

**Vision:** Handoffs should generate automatically without user intervention:
- When a session ends (clean exit, `stop_reason` present), trigger handoff extraction
- When a session has been idle > 30min and has substance (>5 turns, >10 tool calls), offer handoff
- When context% > 90%, generate a checkpoint handoff before compaction
- When resuming a session, auto-surface the previous handoff as reconnection context

**Prerequisites:** I15 (tail coverage), I16/I17 (transaction safety), I18 (status accuracy), I24 (show for active sessions) must all be solid before this can work reliably.

**Implementation ideas:**
1. Post-exit hook: `gaal handoff --auto` triggered by session cleanup
2. Cron-based: periodic scan of recently-ended sessions without handoffs
3. In-session: gaal detects its own session ending and writes the handoff as a final action

**Status:** Deferred until I18-I26 are resolved. Log now so the vision is captured.

---

## I28: `gaal active` shows duplicate entries for same session ID [FIXED 2026-03-10] [REMOVED v0.1.0]

NOTE: `gaal active` command removed in v0.1.0.

**Severity:** High (active view is noisy and misleading)
**Command:** `gaal active -H`

**Problem:** Multiple OS processes can map to the same Claude Code session ID — e.g., three `claude --dangerously-skip-permissions` processes all reconnected/resumed the same session UUID `68b37d6c`. Gaal lists each PID as a separate entry, tripling the session in the output.

**Root cause:** `find_active_sessions()` iterates PIDs and builds one `ActiveSession` per PID. No dedup by session ID happens before output.

**Expected:** Group by session ID. Show one entry per unique session with:
- The most recently active PID (highest CPU, or most recent JSONL event)
- A `pids: [98520, 8009, 40481]` field showing all associated processes
- A `process_count` indicator in human output

**Fix:** After building the `Vec<ActiveSession>`, group by `id`, keep the entry with highest CPU% or most recent action, merge PIDs into a list.

---

## I29: `gaal active` lists child `codex exec` workers as independent sessions [FIXED 2026-03-10]

**Severity:** High (flat list mixes coordinators and their workers)
**Command:** `gaal active -H`

**Problem:** When a parent `codex --yolo` session spawns a child `codex exec` worker via agent-mux, both appear as independent entries in `gaal active`. The user sees 2 sessions when there's really 1 coordinator + 1 worker.

**Evidence:** PID 38425 (`codex --yolo`, parent) and PID 40693 (`codex exec`, child of agent-mux 40661, which is child of 38425). Both map to session `019cd634`.

**Root cause:** No parent-child PID tree walking to detect that one process spawned the other. No session hierarchy in the output.

**Expected:** Detect parent-child via PID tree (`ppid` chain). Child workers should either:
1. Be collapsed under their parent in default view
2. Be shown indented/nested in tree view (`--tree`)
3. Be excluded from default view with `--flat` to see them

**Fix:** After PID discovery, walk `ppid` chain for each process. If a process's ancestor (up to 4 hops) is another discovered session, mark it as a child. Add `parent_pid: Option<u32>` to `ActiveSession`.

---

## I30: `gaal active` shows ghost sessions — pid=0, no live process [FIXED 2026-03-10]

**Severity:** Medium (dead sessions pollute active view)
**Command:** `gaal active -H`

**Problem:** API-discovered Codex sessions (from `~/.codex/sessions/` mtime scan) appear in `gaal active` with `pid=0` even when no actual process exists. These are stale state files from terminated workers that weren't cleaned up.

**Evidence:** Session `3ffb65d8` shows as active with pid=0, `permission_blocked: true`, but no OS process matches.

**Root cause:** `discover_api_active_codex_sessions()` checks JSONL mtime < 5 min but doesn't verify a live process exists for the session.

**Expected:** API-discovered sessions should be cross-checked:
1. If a matching PID is found → real session, merge with PID-discovered entry
2. If no PID and mtime < 2 min → possibly just started, show as `starting`
3. If no PID and mtime > 2 min → dead/ghost, either filter out or mark as `dead`

**Fix:** After API discovery, check each session against the PID-discovered set. If no PID match and mtime > 2 min, exclude or mark status as `dead`.

---

## I31: `gaal active` dedup still not working — same session appears multiple times [FIXED 2026-03-10]

**Severity:** Critical (core UX broken)
**Command:** `gaal active -H`

**Problem:** Despite two fix attempts (commit `9e9ab10` and `f78726f`), session `68b37d6c` (claude) still appears 3 times in `gaal active -H` output. The `dedup_active_output()` function added in `f78726f` is either not being called, not matching IDs correctly, or running at the wrong stage of the pipeline.

**Evidence:** Three entries for `68b37d6c` with identical engine, status, duration, ctx%, last action, stuck signal, and CWD.

**Investigation needed:** Trace the exact pipeline: `find_active_sessions()` → `collect_active()` → `dedup_active_output()` → output. At which stage do 3 entries exist? Does `dedup_active_output()` see the same ID for all 3? Is it being called at all?

---

## I32: `gaal active` stuck detection still wrong — active sessions flagged as stuck [FIXED 2026-03-10]

**Severity:** High (false alerts on healthy sessions)
**Command:** `gaal active -H`

**Problem:** Session `68b37d6c` is the user's active coordinator session (currently running, dispatching agents) but shows as `stuck` with `silence (1033s)`. The session IS producing output — it's literally generating the `gaal active` call. The 1033s silence means the JSONL's last event is ~17 minutes old, but the session has been working through subagents (Agent tool calls) which don't produce JSONL events in the parent session.

Session `019cd71a` (codex) shows as `stuck` with `silence (1180s)` — may be genuinely idle, but the stuck label is too aggressive for a 31-minute session.

**Root cause:** Stuck detection doesn't account for:
1. Sessions currently executing Agent tool_use (subagent dispatch) — parent JSONL goes silent while child works
2. The I19 fix added `executing_agent` detection but it's not working in practice

**Expected:** When last action is `Agent` tool_use without a result, the session is WAITING for a subagent, not stuck. Silence tolerance should be 10x normal (3000s for Claude, 6000s for Codex).

---

## I33: `gaal active` context% wrong — 0% for Claude, 121% for Codex [FIXED 2026-03-10]

**Severity:** Medium (misleading metrics)
**Command:** `gaal active -H`

**Problem:**
- Claude session `68b37d6c` shows 0% context — this is a 22-hour session that has used substantial context. 0% is wrong.
- Codex session `019cd71a` shows 121% context — values above 100% are mathematically impossible and indicate a calculation bug.

**Root cause hypotheses:**
- Claude 0%: Context percentage may not be extractable from Claude JSONL (different event format). The probe function may return 0 when it can't determine context.
- Codex 121%: Token count may include both input and output tokens, or the context window size constant is wrong for the model being used.

**Expected:** Context% should be 0-100%, with `?` or `-` shown when unknown.

---

## I34: `gaal ls` default limit too high — floods output

**Severity:** Medium (UX)
**Command:** `gaal ls` (no flags)

**Problem:** Default `--limit 50` shows too many sessions. For both agents and humans, 50 results is overwhelming. No indication of total count — user doesn't know if they're seeing everything or a subset.

**Expected:** Default limit of 10 with a message: "showing 10 of N sessions — use --limit to see more." JSON mode should include a `total` field.

**Fix:** Change `default_value_t = 50` to `default_value_t = 10` in `src/main.rs`. Add total session count query. In `-H` mode, print footer with count message. In JSON, add `"total": N` alongside `"sessions"`.

---

## I35: `gaal ls -H` and `gaal who -H` — broken table layout with long CWD strings

**Severity:** Medium (UX — tables become unreadable)
**Command:** `gaal ls -H`, `gaal who read ISSUES.md -H`

**Problem:** Deep nested CWD paths like `/Users/otonashi/thinking/building/gaussian-moat-cloud/runs/loops/sqrt36-upper-bound-autoresearch-20260310/agent-mux/candidate-mutation-jlqj2pkf/solver` break table column alignment. The table doesn't survive terminal resize. Same issue affects both `ls -H` and `who -H` output.

**Note:** `gaal active -H` already handles this correctly — it truncates CWD to last 2-3 components with `...` prefix. That truncation logic exists but isn't applied to `ls` and `who` renderers.

**Fix:** Extract the CWD truncation function from `active` and apply it to `ls -H` and `who -H` table renderers. Truncate to last 2-3 path components with `...` prefix. Consider respecting terminal width via `$COLUMNS` or `term_size` crate.

---

## I36: `gaal show -H` still too token-heavy — bypasses summary card [REMOVED v0.1.0]

NOTE: `gaal show` command removed in v0.1.0 - functionality moved to `gaal inspect`.

**Severity:** High (UX + token waste)
**Command:** `gaal show be1c2826 -H`

**Problem:** JSON mode (`gaal show <id>`) returns a reasonable summary card. Human mode (`-H`) bypasses the card and dumps everything — full fact lists, all files, all commands. The user describes it as "madness token-wise." Related to I25 (token-heavy defaults) but specific to the `-H` code path in `show`.

**Expected:** Human mode should be *more* concise than JSON by default, not less. Default `show -H` should render: headline, engine, duration, status, file count, command count, and the path to the full markdown transcript. Full detail only with `--full` / `-F`.

**Fix:** In `src/commands/show.rs`, change the human renderer's default to mirror the JSON summary card. Gate full fact/file/command dumps behind `--full`. Always include the markdown file path so users know where the full transcript lives.

---

## I37: `gaal active` — 3 PIDs dedup bug collapses distinct sessions

**Severity:** High (bug — active view is misleading)
**Command:** `gaal active -H`

**Problem:** Entry `d142e3cc claude idle 7h 27m "boot Jenkins" [3 PIDs]` — the 3 PIDs are actually different sessions that happen to start with the same prompt. They're being collapsed because dedup is matching on prompt similarity rather than session ID. This is the inverse of I28/I31 (duplicate entries) — now sessions are being over-merged.

**Root cause:** The dedup logic introduced to fix I28/I31 likely keys on headline/prompt text rather than session UUID. Sessions with similar initial prompts (e.g., multiple "boot Jenkins" sessions) get merged into one entry.

**Expected:** Dedup must key strictly on session ID. Three different session IDs → three entries, regardless of prompt similarity.

**Fix:** In `dedup_active_output()`, verify the dedup key is the session `id` field, not headline/prompt/CWD. If merging is happening on a compound key that includes prompt, remove the prompt component.

---

## I38: `gaal who wrote ISSUES.md` — JSON mode massively too many tokens

**Severity:** High (token waste — blows agent context)
**Command:** `gaal who wrote ISSUES.md`

**Problem:** JSON mode dumps all facts for all matching sessions — massive token output. In contrast, `gaal who wrote ISSUES.md -H` is focused and readable. JSON default should be equally concise.

**Sub-issue:** `-H` mode is missing date range information — shows which sessions wrote the file but not *when* the writes happened.

**Fix:**
1. JSON default for `who` should return summary records: `{session_id, engine, date, headline, fact_count}` — not the raw fact arrays.
2. Add `--verbose` / `--full` flag to include raw facts when needed.
3. Add `first_seen` / `last_seen` timestamps to both JSON and `-H` output for when the matching operations occurred.

---

## I39: `gaal who` returns no results for files that were definitely worked on

**Severity:** High (bug — trust issue)
**Command:** `gaal who read CLAUDE-QMS.md` (at sorbent-demo), `gaal who wrote AUDIT_PHASE3.md`

**Problem:** Both queries return zero results despite these files being actively worked on by sessions. Also, `gaal who wrote CLAUDE.md` at gaal/ may be returning inaccurate results — user recalls only one session writing that file.

**Root cause hypotheses:**
1. **Indexing gap:** Sessions that worked on these files were never indexed (possible I17 backfill crash for subagent-heavy sessions).
2. **Parser gap:** Certain file operation event types not captured as facts during indexing.
3. **CWD filtering:** If `--cwd` is implicitly set to current directory, sessions working from a different CWD would be filtered out even if they touched the same files.

**Investigation needed:**
- Pick a session known to have read CLAUDE-QMS.md. Check if it exists in the DB (`gaal show <id>`).
- If it exists, check if its file_read facts are in the facts table.
- If facts are missing, trace the parser path for file_read events.
- Check whether `--cwd` auto-filtering is happening.

---

## I40: `gaal who ran` — false positive matches via substring in long command strings

**Severity:** High (bug — search results are misleading)
**Command:** `gaal who ran tortuise -H`, `gaal who ran tortui -H`

**Problem:** Both queries return the same codex sessions from yesterday. "tortuise" hasn't been run in 7+ days, and "tortui" doesn't exist as a tool at all. The matching is happening via substring within long command strings (e.g., a `bash` command that mentions "tortoise" somewhere in its arguments), not on the actual tool/command name.

**Expected:** `gaal who ran X` should match on the tool/command name (first token or primary executable), not on arbitrary substrings within full command arguments. A query for "tortuise" should not match a session that ran `bash -c "... tortoise ..."`.

**Fix:**
1. For `ran` verb, extract and match against the command name (first token of the command string), not the full argument string.
2. Add `--fuzzy` flag to enable substring matching when the user explicitly wants it.
3. Consider: if exact match returns 0 results, suggest `--fuzzy` in the error message.

---

## I41: `gaal who -H` needs limit indicator message

**Severity:** Medium (UX)
**Command:** `gaal who ran python3 -H`

**Problem:** Shows 10 results (the default limit) but no indication there are more. User can't tell if there are exactly 10 matching sessions or 100 with only 10 shown.

**Fix:** Same pattern as I34 — add "showing N of M results — use --limit N for more" footer to `-H` output. In JSON, add `"total"` field.

---

## I42: `gaal who` verb help text — `touched` and `changed` are unclear

**Severity:** Medium (UX — discoverability)
**Command:** `gaal who touched`, `gaal who changed`

**Problem:** The `touched` and `changed` verbs have no documentation. User doesn't understand what they match. The `--help` text just says "Action verb" without listing or explaining the available verbs.

**Fix:** Add a verb reference to `gaal who --help` and to the error message when an invalid verb is used:
```
Verbs:
  read       sessions that read a file (file_read)
  wrote      sessions that wrote/created a file (file_write)
  ran        sessions that executed a command (command_run)
  touched    sessions that read OR wrote a file
  changed    sessions that wrote OR deleted a file
  installed  sessions that ran install commands
  deleted    sessions that deleted a file
```

---

## I43: `gaal who` with no args should show help, not error [FIXED v0.1.0]

**Severity:** Low (UX)
**Command:** `gaal who` (no args)

**Problem:** Running `gaal who` with no arguments produces an error. Should show the help menu with verb reference instead.

**Fix:** Make `verb` optional in the CLI definition. If verb is None, print the verb reference table (I42) and exit 0. Alternatively, use clap's built-in behavior to show subcommand help.

---

## I44: Rename `gaal find` — name too generic for salt-specific function

**Severity:** Low (UX — naming confusion)
**Command:** `gaal find`

**Problem:** `gaal find` only finds JSONL files by salt token, but the name implies general-purpose file finding. Users encountering `gaal find` in help text will expect `find . -name "*.md"` behavior.

**Fix:** Rename to `find-salt` (preferred — matches the salt/find-salt workflow). Keep `find` as a hidden alias for backward compatibility (`#[command(alias = "find")]`). Update help text: "Find the JSONL file containing a salt token generated by `gaal salt`."

---

## I45: Rename `gaal handoff` — bare noun is ambiguous

**Severity:** Low (UX — naming)
**Command:** `gaal handoff`

**Problem:** `gaal handoff` could mean show, create, list, or delete a handoff. The command generates a new handoff via LLM extraction, which is a specific action that should be named as a verb.

**Options:**
1. `gaal gen-handoff` — clearest for the generation action
2. `gaal create-handoff` — more standard
3. Make `handoff` a subcommand group: `gaal handoff generate`, `gaal handoff show`, `gaal handoff list`

**Fix:** Option 3 is future-proof if we want `gaal handoff show <id>` and `gaal handoff list --since 7d` later. For now, rename to `gen-handoff` with `handoff` as hidden alias.

---

## I46: Rethink `show` vs `inspect` — what does an agent actually need?

**Severity:** Medium (architecture — design needed)

**Problem:** User insight: "I would be very much against dumping whole session summary in CLI tool response. Much better logic — point to the file PATH for the full session transcript and notify that it's token heavy."

**Current state:**
- `show` = full session record (facts, files, commands, errors, git ops)
- `inspect` = operational health snapshot (context%, velocity, stuck signals)

**Gap:** Neither command gives what an agent actually wants: a brief card with just enough to decide whether to dig deeper, plus a path to the full transcript for delegation.

**Design questions:**
1. What signal does `show` give beyond `ls`? → files read/written, commands, errors, duration breakdown
2. Should `show` default to a "session card" (headline + metadata + transcript path)?
3. Should full dump be behind `--full` only?
4. Does `inspect` need to exist separately or can it fold into `show --health`?

**Status:** Post-release design work. I36 is the immediate fix (make show -H brief by default).

---

## I47: `gaal who -H` results missing date ranges

**Severity:** Low (enhancement)
**Command:** `gaal who wrote ISSUES.md -H`

**Problem:** Output shows which sessions wrote the file but not when. Date/time of the actual file operations would make results much more actionable — user can see "session X wrote ISSUES.md 3 days ago" vs "session Y wrote it 2 hours ago."

**Fix:** Include `first_seen` and `last_seen` timestamps for the matching facts in both JSON and `-H` output. Source from the fact's `created_at` or the session event timestamp.

---

## I48: Token counting accuracy — input + cache_creation + cache_read [FIXED v0.1.0]

**Severity:** High (metrics accuracy)
**Problem:** Token counting was inaccurate, not properly accounting for cache read/write operations.
**Fix:** Token counting now properly includes input tokens + cache_creation + cache_read for accurate session cost tracking.

---

## I49: Error classification accuracy — only shell tools with exit_code != 0 [FIXED v0.1.0]

**Severity:** High (error classification accuracy)
**Problem:** Error classification was too broad, flagging non-error conditions as errors.
**Fix:** Error classification now only flags shell tools (bash commands) with non-zero exit codes as errors.

---

## I50: `gaal show` / `gaal inspect` command merge [FIXED v0.1.0]

**Severity:** High (command consolidation)
**Problem:** `gaal show` and `gaal inspect` provided overlapping functionality with confusion over which to use.
**Fix:** `gaal show` command removed, functionality merged into `gaal inspect` as the single session detail command.

---

## I51: SessionStatus / status field removal [FIXED v0.1.0]

**Severity:** High (data model simplification)
**Problem:** SessionStatus enum and status taxonomy provided little value, mostly noise.
**Fix:** SessionStatus enum and related status fields removed from data model in v0.1.0.

---

## I52: `gaal ls` envelope format with query_window [FIXED v0.1.0]

**Severity:** Medium (output format standardization)
**Problem:** `gaal ls` output format inconsistent.
**Fix:** `gaal ls` now returns standardized envelope format with query_window metadata.

---

## I53: `gaal who` codex subjects path cleaning [FIXED v0.1.0]

**Severity:** Medium (data quality)
**Problem:** Codex subjects contained patch strings instead of clean file paths.
**Fix:** Codex subjects are now cleaned to contain proper file paths, not patch diff strings.

---

## I54: `gaal tag ls` subcommand addition [FIXED v0.1.0]

**Severity:** Low (feature completeness)
**Problem:** No way to list existing tags.
**Fix:** Added `gaal tag ls` subcommand to list all available tags.

---

## I38: `gaal who` default output token-heavy JSON [FIXED 2026-03-11]

**Severity:** High (token waste)
**Command:** `gaal who wrote ISSUES.md`

**Problem:** Default JSON output dumped full `detail` field per fact (containing entire file contents or edit JSON). A simple `who wrote ISSUES.md` returned 41KB of JSON.

**Fix:** Default output now groups facts by session, showing session_id, engine, latest_ts, fact_count, and truncated subjects list (~400 bytes). `--full/-F` restores per-fact output with full detail fields. Human mode (`-H`) also defaults to grouped brief view.

---

## I39: `gaal who` missing results for sorbent-demo files — RESEARCH COMPLETE

**Severity:** High (trust-critical)
**Command:** `gaal who read CLAUDE-QMS.md`, `gaal who wrote AUDIT_PHASE3.md` at sorbent-demo

**Root causes (three independent issues):**

1. **Time window too narrow:** Default `--since 7d` excludes files worked on weeks ago. The sorbent-demo sessions with these files date from early February (35+ days ago). Using `--since 60d` finds AUDIT_PHASE3.md results.

2. **Fact type gap:** `who read` only matches `file_read` facts (Read tool), `who wrote` only matches `file_write` facts (Write tool). Files read/written via bash commands (`cat`, `sed`, `echo >`) are stored as `command` facts, not file_read/file_write. CLAUDE-QMS.md was edited via bash (`git add CLAUDE-QMS.md`) and never via the Write tool — so `who wrote` can never find it.

3. **Subagent JSONL not indexed:** Many file operations happened in Claude Code subagent sessions stored under `~/.claude/projects/<hash>/<session>/subagents/agent-*.jsonl`. The discovery function `collect_project_jsonl_files()` in `src/discovery/claude.rs` only scans one level deep (`<hash>/*.jsonl`), missing all subagent files two levels deeper.

**Evidence:**
- `sqlite3 ~/.gaal/index.db "SELECT ... WHERE subject LIKE '%CLAUDE-QMS%'"` → only `command` and `error` facts, zero `file_write` facts
- `grep -rl "CLAUDE-QMS" ~/.claude/projects/` → found in subagent JSONLs under `subagents/` dirs
- `gaal who read AUDIT_PHASE3.md --since 60d` → found session `6f8db90f` from 2026-02-05

**Proposed fixes:**
- Issue 1: No code change needed — users should adjust `--since` for older files
- Issue 2: Consider adding a `who created` or `who modified` verb that also searches `command` facts for file-manipulating bash commands
- Issue 3: Extend `collect_project_jsonl_files()` to recurse into `subagents/` directories. This would significantly increase index coverage.

---

## I40: `gaal who ran` false positives from substring matching [FIXED 2026-03-11]

**Severity:** High (trust — incorrect results)
**Command:** `gaal who ran tortuise -H`, `gaal who ran tortui -H`

**Problem:** Both returned codex sessions from yesterday. "tortuise" hadn't been run for 7+ days. "tortui" doesn't exist as a tool. The matches were against long bash commands that enumerated project files — e.g., `for f in ... projects/tortuise.md ...` where "tortuise" appeared as a file path argument, not as a command being executed.

**Root cause:** `who ran` used `contains_ci()` to match the search term against the entire `detail` field (full bash command string). Any substring match anywhere in arguments, file paths, or piped commands would trigger a hit.

**Fix:** Introduced `MatchMode::CommandName` which extracts program names from shell command strings by splitting on `&&`, `||`, `|`, `;` and taking the first token of each segment (skipping variable assignments and env prefixes like `sudo`, `env`, etc.). The search term now matches only against extracted command names, not the full argument string.

---

## I55: Fact extraction pipeline produces lossy context for create-handoff

**Severity:** Medium (workaround: session.md transcripts now used directly)
**File:** `src/commands/handoff.rs` (`build_context()`)

**Problem:** `build_context()` constructs LLM context from DB facts: commands (cap 40), errors (cap 20), file ops (cap 40), decisions (cap 20), each truncated to 400 chars. This produces ~3.5k tokens from sessions that may span 190+ turns.

Conversational decisions (e.g., "delete gaal active", "merge show into inspect") are NOT indexed as facts because they happen in assistant replies, not tool calls. The fact extractor only captures tool invocations — so strategic reversals mid-session are invisible to `build_context()`.

Result: the handoff LLM sees early-session state but misses late-session reversals, producing factually wrong handoffs. Discovered when session `75b8d982` (70hr, 190 turns) handoff described deleted features as still existing.

**Workaround applied:** `create-handoff` now reads full session.md transcripts instead of fact-based context.

**Action:** Audit fact extraction code. Determine whether fact-based context should be improved (new fact types for decisions) or removed entirely in favor of session.md. May be able to simplify handoff.rs significantly.

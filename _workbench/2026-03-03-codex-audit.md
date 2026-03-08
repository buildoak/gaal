# Gaal Spec Audit (2026-03-03)

Canonical spec: `DESIGN.md` (design-v3)
Audit scope: all `src/**/*.rs` files and `src/db/schema.sql`

## Pass Checklist

- [x] `gaal ls` JSON schema fields match the spec example (`id`, `engine`, `model`, `status`, `cwd`, `started_at`, `ended_at`, `duration_secs`, `parent_id`, `child_count`, `tokens`, `tools_used`, `headline`) in [src/commands/ls.rs](src/commands/ls.rs:95).
- [x] `gaal inspect` JSON schema matches the spec structure (including `process`, `context`, `current_turn`/`last_turn`, `velocity`, `stuck_signals`, `recent_errors`) in [src/commands/inspect.rs](src/commands/inspect.rs:52).
- [x] `gaal who` JSON schema fields match spec (`session_id`, `engine`, `ts`, `fact_type`, `subject`, `detail`, `session_headline`) in [src/commands/who.rs](src/commands/who.rs:60).
- [x] `gaal search` JSON schema fields match spec (`session_id`, `engine`, `turn`, `fact_type`, `subject`, `snippet`, `ts`, `score`, `session_headline`) in [src/commands/search.rs](src/commands/search.rs:79).
- [x] `gaal index status` output field names match the spec example in [src/commands/index.rs](src/commands/index.rs:154).
- [x] SEARCH uses Tantivy BM25-style ranked retrieval (Tantivy `QueryParser` + `TopDocs`) in [src/commands/search.rs](src/commands/search.rs:216) and [src/commands/search.rs](src/commands/search.rs:224).
- [x] SQL data model in `schema.sql` matches DESIGN.md table/column/index definitions (sessions, facts, handoffs, session_tags + indexes) in [src/db/schema.sql](src/db/schema.sql:1).

## Discrepancies (Spec vs Implementation)

### Critical

- **`gaal show` default JSON shape diverges from spec and is not a full session record by default.**
  - Spec example presents full record including `files`, `commands`, `errors`, `git_ops`; implementation strips these unless explicit flags are set.
  - Evidence: [src/commands/show.rs](src/commands/show.rs:390), [src/commands/show.rs](src/commands/show.rs:397).

- **`gaal show` emits extra fields not in spec example (`child_count`, `last_event_at`, `exit_signal`, `tags`) and tree mode shape differs.**
  - `SessionRecord` includes non-spec fields and is serialized directly.
  - `--tree` adds a nested `tree` field instead of returning the tree schema shown in spec.
  - Evidence: [src/model/session.rs](src/model/session.rs:27), [src/model/session.rs](src/model/session.rs:49), [src/model/session.rs](src/model/session.rs:53), [src/model/session.rs](src/model/session.rs:55), [src/commands/show.rs](src/commands/show.rs:378), [src/commands/show.rs](src/commands/show.rs:423).

- **Exit code semantics are not consistently enforced (`1 = no results`).**
  - `ls`, `who`, `search`, `active`, and `inspect --active/--ids` can return empty JSON with exit code 0 instead of erroring with `NoResults`.
  - Evidence: [src/commands/ls.rs](src/commands/ls.rs:167), [src/commands/who.rs](src/commands/who.rs:126), [src/commands/search.rs](src/commands/search.rs:128), [src/commands/active.rs](src/commands/active.rs:131), [src/commands/inspect.rs](src/commands/inspect.rs:141).

- **Status computation is inconsistent with spec in `ls`/`show`.**
  - `ls` stuck detection is silence-only (no loop/context/permission) and completion/failure is driven by DB fields, not full runtime determination.
  - `show` status is simplified to `active/completed/failed` and never computes `idle/stuck/unknown` from live signals.
  - Evidence: [src/commands/ls.rs](src/commands/ls.rs:454), [src/commands/ls.rs](src/commands/ls.rs:485), [src/commands/show.rs](src/commands/show.rs:674).

- **Stuck detection does not match the spec exactly.**
  - Permission-blocked is implemented as “any unresolved tool call in tail window”, not “last JSONL record is tool_use with no subsequent tool_result”.
  - Context exhaustion uses `>= 95%` instead of strict `> 95%`.
  - `ls` stuck path ignores loop/context/permission signals.
  - Evidence: [src/commands/active.rs](src/commands/active.rs:268), [src/commands/active.rs](src/commands/active.rs:366), [src/commands/active.rs](src/commands/active.rs:653), [src/commands/ls.rs](src/commands/ls.rs:485).

- **Search indexing pipeline is incomplete: backfill does not build/update Tantivy index.**
  - BM25 search code exists, but no command path invokes index build after backfill/reindex; `search` can fail with `NoIndex` despite DB being populated.
  - Evidence: [src/commands/search.rs](src/commands/search.rs:132), [src/commands/index.rs](src/commands/index.rs:101), [src/commands/search.rs](src/commands/search.rs:395).

### Medium

- **`gaal recall` summary schema misses `handoff_path` from the spec example.**
  - `RecallSummary` has no `handoff_path` field.
  - Evidence: [src/commands/recall.rs](src/commands/recall.rs:68).

- **Recall scoring is close but not exact to spec formula.**
  - Recency uses `age_days = (today - session_date) + 1`, shifting age by +1 day.
  - Query tokenizer applies an extra minimum-token-length gate (`len < 3` unless known project), which is not in spec.
  - Evidence: [src/commands/recall.rs](src/commands/recall.rs:248), [src/commands/recall.rs](src/commands/recall.rs:496).

- **WHO semantic behavior does not fully match spec table for `changed`/`deleted`.**
  - `changed` matching is subject-based; no dedicated git-commit-file-touch matching behavior.
  - `deleted` does not implement spec’s empty-write deletion signal.
  - Evidence: [src/commands/who.rs](src/commands/who.rs:169), [src/commands/who.rs](src/commands/who.rs:173), [src/commands/who.rs](src/commands/who.rs:205).

- **`inspect --tag` is present at CLI parse layer but explicitly rejected (not wired).**
  - Spec utility section states `--tag` filter should be available on inspect.
  - Evidence: [src/main.rs](src/main.rs:394), [src/main.rs](src/main.rs:397).

- **`gaal active` emits nullable `id`, but spec example requires `id` string.**
  - Evidence: [src/commands/active.rs](src/commands/active.rs:43), [src/commands/active.rs](src/commands/active.rs:219).

### Low

- **Potential spec/example mismatch around `gaal ls --sort cost` support.**
  - `commands::ls` supports `Cost`, but top-level CLI enum/mapper does not expose it.
  - Evidence: [src/commands/ls.rs](src/commands/ls.rs:90), [src/main.rs](src/main.rs:276), [src/main.rs](src/main.rs:565).

- **No-dead-code / no-unused-dependency goal is not fully met.**
  - Clippy reports unused crate dependencies for bin target and shows public functions that are currently unreferenced in command flow.
  - Evidence: [src/commands/search.rs](src/commands/search.rs:132), [src/commands/search.rs](src/commands/search.rs:189), [Cargo.toml](Cargo.toml:14).

## Requirement-by-Requirement Verdict

1. Verb output schemas: **PARTIAL FAIL** (`show`, `recall`, `active` discrepancies).
2. Flags in `main.rs` and wired: **PARTIAL FAIL** (`inspect --tag` rejected; possible `ls --sort cost` gap).
3. Exit codes: **FAIL** (no-results handling inconsistent).
4. Status computation: **FAIL** (especially `ls`/`show`).
5. Stuck detection: **FAIL** (permission/context threshold/ls behavior deviations).
6. Recall scoring exactness: **PARTIAL FAIL** (age/tokenization deviations).
7. Who verb expansion table: **PARTIAL FAIL** (`changed`/`deleted` semantics incomplete).
8. Search BM25 via Tantivy: **PASS** (engine), **but integration gap** (index build path missing).
9. No dead code / no unused deps: **FAIL**.
10. Data model in `schema.sql`: **PASS**.

# CLAUDE.md — gaal

Session observability CLI for Claude Code and Codex. Rust, single binary, macOS-first.
Version 0.1.0. Read this file before writing any code.

## Architecture

### Module Structure

```
src/
  main.rs              CLI entry point (clap derive)
  lib.rs               Crate root, re-exports
  config.rs            Config loading (~/.gaal/config.toml)
  error.rs             AX-compliant error types with format_human()
  util.rs              Shared utilities

  commands/            One file per command
    ls.rs              Fleet view over indexed sessions
    inspect.rs         Session detail view (replaced `show`)
    who.rs             Inverted attribution queries (read/wrote/ran/touched/changed/deleted)
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

### Data Flow

```
JSONL files on disk
  → discovery/ (find files in ~/.claude/projects/ and ~/.codex/)
  → parser/ (dual Claude/Codex parsers → events → facts)
  → db/ (SQLite for structured data + Tantivy for FTS)
  → commands/ (query DB and Tantivy)
  → output/ or render/ (JSON, human tables, or markdown)
```

### Session Types

| Type | Meaning |
|------|---------|
| `standalone` | Normal session, no subagents |
| `coordinator` | Parent session that spawned subagents via Agent tool |
| `subagent` | Child session spawned by a coordinator |

### Two-Source Subagent Model

Subagent data comes from two complementary sources:

1. **Parent JSONL `toolUseResult` blocks** — fleet index. Provides `agentId`, `totalTokens`, `totalDurationMs`, `totalToolUseCount`, `status`, `prompt/description`. Fast, always available if parent exists.
2. **Subagent JSONL files** (`{session_dir}/subagents/agent-{agentId}.jsonl`) — detail store. Full turn-by-turn trace, every tool call, every file read/write. Needed for `who`, `search`, and transcript rendering.

Path from parent to subagent file is deterministic:
`Parent JSONL → toolUseResult.agentId → {session_dir}/subagents/agent-{agentId}.jsonl`

### Storage Layout

```
~/.gaal/
  index.db                                    SQLite database
  tantivy/                                    Full-text search index
  config.toml                                 Runtime config
  prompts/handoff.md                          Handoff extraction prompt
  data/
    claude/
      sessions/YYYY/MM/DD/<id>.md             Rendered transcripts
      handoffs/YYYY/MM/DD/<id>.md             Generated handoffs
    codex/
      sessions/YYYY/MM/DD/<id>.md
      handoffs/YYYY/MM/DD/<id>.md
```

## v0.1.0 Scope

Eleven commands. Core session observability without monitoring features.

| # | Command | What it does |
|---|---------|-------------|
| 1 | `gaal ls` | Fleet view. List sessions with envelope format, query_window, filters. |
| 2 | `gaal inspect <id>` | Session detail. Files, commands, timeline, git ops, token breakdown. |
| 3 | `gaal who <verb> <target>` | Inverted attribution. Find sessions by file/command activity. |
| 4 | `gaal search <query>` | BM25 full-text search over indexed facts via Tantivy. |
| 5 | `gaal recall [topic]` | Ranked retrieval over generated handoffs for session continuity. |
| 6 | `gaal transcript <id>` | Session transcript markdown — path metadata or `--stdout` dump. |
| 7 | `gaal create-handoff <id>` | LLM-powered handoff generation via agent-mux dispatch. |
| 8 | `gaal salt` | Generate unique salt token for self-identification. |
| 9 | `gaal find-salt <token>` | Find JSONL file by salt token. Content-addressed discovery. |
| 10 | `gaal index <sub>` | Index maintenance: backfill, reindex, prune, import-eywa, status. |
| 11 | `gaal tag` | Tag management: add, remove, ls. |

**What's in:** Query, search, inspect, transcript, handoff generation, self-identification, tagging, subagent indexing/attribution, dual Claude+Codex support, AX error messages.

**What's out:** Process monitoring, live tailing, real-time status, cross-session injection, daemon mode, web UI. See Feature Kill List.

## Feature Kill List (permanent)

These are **deleted, not deferred**. Do not re-implement.

| Feature | Why killed |
|---------|-----------|
| `gaal active` command | Process monitoring too fragile. Removed in v0.1.0. |
| `gaal show` command | Merged into `inspect`. Redundant commands removed. |
| `SessionStatus` enum | Status taxonomy was noise. Removed in v0.1.0. |
| `--live`, `--watch`, `--active` flags | Real-time monitoring features removed. |
| Velocity, context %, recent_errors fields | Heuristic calculations were unreliable. |
| Process blocks in output | No process monitoring in v0.1.0. |
| Stuck detection | Heuristic garbage. Wrong more than right. 50+ edge cases for near-zero value. |
| Parent-child linking via PID | 1 out of 2,433 sessions ever linked. Dead feature. Salt-based discovery replaced it. |
| Loop detection | Premature. Insufficient signal in JSONL to detect reliably. |

If you find yourself re-adding any of these: stop, re-read this section, and ask yourself why you think you'll succeed where 5+ attempts failed.

## Verification Protocol

**This is law. Every fix, every feature, every PR.**

### Before writing code

1. **Dump real data first.** `grep`, `jq`, `head` on actual JSONL files. See real field names, real structures, real edge cases.
2. **Never reason about what JSONL "should" contain.** Claude and Codex schemas are undocumented and change without notice. The only source of truth is the bytes on disk.
3. **Test your assumptions.** If you think a field is called `content`, grep for it. If you think events have a `type` field, prove it.
4. **Read DOCS.md** for the command you are modifying. It is the canonical reference for flags, output shapes, and behavior.
5. **Check BACKLOG.md** for context on the item you are working on. It records what shipped, what's open, and what's a dead end.

### While writing code

6. **One fix per commit.** No bundling. If fix A breaks, you can revert without losing fix B.
7. **Match code to reality, not reality to code.** If the JSONL has `arguments` in one place and `input` in another, handle both. Don't normalize upstream data you don't control.

### After writing code

8. **`cargo build --release` — always.** The installed binary is a symlink to `target/release/gaal`. Debug builds don't update it. If you run `cargo build` without `--release`, your fix doesn't take effect. This has burned us multiple times.
9. **Test against live sessions.** Run the built binary against real JSONL files on this machine. Not mocks, not synthetic data. Use `gaal ls --limit 3` and `gaal inspect latest` as smoke tests.
10. **Verify the output.** Don't assume "it compiled, so it works." Run the command, read the output, confirm it matches what the real data says.

### The evidence-first rule

When debugging: **dump first, code second.**

The pattern that works:
```
grep real files → see actual field names → fix code to match reality → cargo build --release → test with real binary
```

The pattern that fails:
```
read Rust source → reason about what "should" work → write fix → cargo build (debug) → wonder why nothing changed
```

## AX Testing Convention

Gaal uses a three-layer acceptance testing harness. **Every change to the CLI surface must update the relevant layer.**

### Layer 1: Error Paths (`tests/ax/layer1-errors/`)

**Every new command, flag, or error path MUST have entries in `tests/ax/layer1-errors/`** covering:
- Missing required arguments
- Invalid argument values
- Not-found targets
- Ambiguous IDs
- Missing index

Each entry verifies:
- The correct non-zero exit code is returned
- The error message includes a working example of the correct command
- The error message names all valid options/values when the input was wrong
- The error follows the JSON format: `{"error": "...", "hint": "...", "example": "..."}`

### Layer 2: Task Workflows (`tests/ax/layer2-tasks/`)

**Every new user-facing workflow SHOULD have a task entry** covering the happy path end-to-end. A task is a sequence of gaal commands that an agent would run to accomplish a real goal (e.g., "find who last edited a file and inspect that session").

### Layer 3: Analysis (`tests/ax/layer3-analysis/`)

Quality analysis over the full error corpus. Checks that error messages are learnable — an agent encountering the error for the first time can self-correct from the message alone.

### Error Message Requirements

All user-facing error messages MUST follow AX principles:

1. **Include a working example** of the correct command the user can copy
2. **Name all valid options/values** when the input was wrong (e.g., list valid verbs for `who`, valid sort fields for `ls`)
3. **Use the structured error format** in `format_human()`:
   ```
   What went wrong: <specific problem>
   Example: <correct invocation>
   Hint: <what to try next>
   ```
4. **Exit codes are meaningful:** 0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error

### Running AX Tests

```bash
# Run the existing integration test suites
cd /Users/otonashi/thinking/building/gaal
./tests/run-all.sh

# Individual suites (suite-1 through suite-8)
./tests/suite-1.sh
```

The integration suites (`tests/run-all.sh`, `tests/suite-*.sh`) are the current quality gate. They test all 11 commands against live data on this machine. The AX layer harness (`tests/ax/`) is the structured extension point — new error paths and workflows go there.

### What "AX-compliant" Means in Practice

The `-H` flag routes errors through `format_human()` in `src/error.rs`. Every `GaalError` variant has a command-specific error message with what/example/hint. When adding a new error path:

1. Add a match arm in `format_human()` (or the relevant helper: `no_results_message`, `not_found_message`, `parse_error_message`)
2. Add integration test coverage in `tests/run-all.sh` or `tests/ax/layer1-errors/`
3. Verify by running the bad command and reading stderr

## Token Accounting

- **Cache tokens** are fully tracked: `cache_read_tokens` and `cache_creation_tokens` stored in DB, surfaced in `inspect --tokens`, and included in transcript frontmatter.
- **Peak context** = max(input_tokens + cache_read + cache_creation) across all turns. Represents actual API context window usage per turn. This is NOT file size.
- **Model-aware cost estimation:** `estimate_session_cost()` uses per-model pricing (Opus $15/$75, Sonnet $3/$15, Codex $2/$8 per Mtok). Cache read and creation tokens are priced separately.
- **Tool counting:** Both Claude and Codex tool uses are counted. Claude tools appear as `ContentBlock::ToolUse` inside `AssistantMessage` events. Codex tools appear as standalone `EventKind::ToolUse` events. Both paths increment `total_tools`.
- **Usage deduplication:** Claude uses `dedup_key` (message ID) to avoid double-counting. Codex uses cumulative `total_tokens` as dedup key to handle rate-limit bucket duplicates.
- **Error deduplication:** Error facts are keyed by `tool:{tool_use_id}` or `ts:{timestamp}|exit:{code}` to prevent double-counting.

## Coding Conventions

### Rust Style

- Edition 2021. Stable toolchain.
- `cargo fmt` before every commit. `cargo clippy` clean.
- Prefer `thiserror` for error types, `anyhow` for propagation in non-library code.
- Prefer `match` over chains of `if let`. Exhaustive matching — no wildcard arms on enums you control.
- Public API types live in `src/model/`. Internal types stay in their module.

### Naming

- Commands: snake_case file names matching the CLI subcommand (`ls.rs`, `find.rs`, `inspect.rs`).
- Types: `SessionRow`, `FactRow`, `HandoffRow` — the `Row` suffix marks DB-backed types.
- Parser types: `EventKind`, `ContentBlock` — semantic names, not schema mirrors.

### Dependencies

Minimize. Prefer std. Current deps are intentional:
- `clap` (derive) for CLI parsing
- `serde` + `serde_json` for serialization
- `chrono` for timestamps
- `rusqlite` (bundled) for SQLite
- `tantivy` for FTS
- `anyhow` + `thiserror` for errors
- `toml` for config
- `dirs` for platform paths
- `regex` for pattern matching
- `rand` for salt generation
- `terminal_size` for adaptive formatting

Do not add new dependencies without justification. No async runtime — gaal is synchronous by design.

### Git

- One fix per commit. Atomic, bisect-friendly.
- Commit message format: `type(scope): description` (e.g., `fix(parser): handle missing content field in Codex events`)
- Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`
- `Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>` on agent-authored commits
- Always `--release` build before treating a commit as shipped

## Key Paths

| Path | What |
|------|------|
| `CLAUDE.md` | This file. Worker orientation. |
| `DOCS.md` | Canonical user documentation. Command reference with real examples. |
| `BACKLOG.md` | Open items, shipped items, design notes. Check before starting work. |
| `skill/SKILL.md` | What calling agents see. The external contract. |
| `Cargo.toml` | Version 0.1.0. Dependencies. |
| `src/error.rs` | AX-compliant error types. `format_human()` is where error messages live. |
| `src/commands/` | One file per command. Start here when modifying a command. |
| `src/parser/facts.rs` | Fact extraction from parsed events. Tool counting, error dedup, peak context. |
| `src/parser/claude.rs` | Claude JSONL parser. Handles Claude-specific event schema. |
| `src/parser/codex.rs` | Codex JSONL parser. Handles Codex-specific event schema. |
| `src/db/schema.rs` | SQLite schema + migrations. `savepoint_with_name()` for nested transactions. |
| `src/render/session_md.rs` | JSONL-to-markdown renderer. Frontmatter, turns, subagent summaries. |
| `src/subagent/` | Subagent discovery, parent parsing, indexing engine. |
| `tests/run-all.sh` | Integration test runner. 8 suites, ~60 tests against live data. |
| `tests/ax/` | AX acceptance test harness (layer1-errors, layer2-tasks, layer3-analysis). |

## What Workers Must Do Before Writing Code

1. **Read this CLAUDE.md.** You are doing this now. Good.
2. **Read DOCS.md** for the command you are modifying. It has the exact flags, output shapes, and real examples.
3. **Run the command manually** to see current behavior. Use `gaal <cmd> --help` and then run it against live data.
4. **Check BACKLOG.md** for context on the item you are working on. It records what shipped, what failed, and what's a dead end.
5. **Grep real JSONL** before assuming field names or event structures. Evidence first.
6. **Build with `--release`** and verify with the real binary. Every time.

## Build

```bash
cargo build --release
# Binary: target/release/gaal (symlinked to ~/.cargo/bin/gaal)
```

Clean build: ~8 min. Incremental: ~30s. Always use `--release`.

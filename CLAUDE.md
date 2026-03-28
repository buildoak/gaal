# CLAUDE.md — gaal

Session observability CLI for Claude Code and Codex. Rust, single binary, macOS-first.

## v0.1.0 Scope (public release)

Eleven commands. Core session observability without monitoring features.

| Command | What it does |
|---------|-------------|
| `gaal inspect <id>` | Session detail view. Files, commands, timeline, git ops, token breakdown. |
| `gaal ls` | List sessions with envelope format and query_window. |
| `gaal who <verb> <target>` | Find sessions by file/command activity. Query_window support. |
| `gaal recall <topic>` | Ranked retrieval for session continuity. Handoff files + JSONL fallback. |
| `gaal search <query>` | Find sessions by content. BM25 ranked via Tantivy. |
| `gaal create-handoff <id>` | Generate handoff document via LLM extraction (agent-mux dispatch). |
| `gaal transcript <id>` | Session transcript markdown — path metadata or `--stdout` dump. |
| `gaal salt` | Generate unique token for self-identification. |
| `gaal find-salt <token>` | Find JSONL file by salt token. |
| `gaal index` | Index JSONL files and tag management. |
| `gaal tag` | Tag management with 'tag ls' subcommand. |

### Session Detection: Salt-Based Strategy

**Self-identification**: `gaal salt` + `gaal find-salt SALT` — content-addressed detection. A unique salt token is printed into the session JSONL (via tool-result), then grepped to find the file. No PIDs, no process trees. Works through subagent indirection, broken process ancestry, and concurrent sessions.

This strategy enables reliable session discovery from within sessions without external process monitoring.

### Self-Handoff Protocol (from inside a session)

```bash
# Step 1: Generate + embed salt (must be separate Bash tool calls for JSONL flush)
SALT=$(gaal salt)
echo "$SALT"

# Step 2: Find own JSONL
JSONL=$(gaal find-salt "$SALT" | jq -r .jsonl_path)

# Step 3: Generate handoff
gaal create-handoff --jsonl "$JSONL"
```

Steps 1 and 2 MUST be separate tool invocations — the JSONL flush happens between calls. The salt appears in the tool-result of step 1, and `gaal find-salt` scans for it in step 2.

## Feature Kill List (permanent)

These are **deleted, not deferred**. Do not re-implement.

| Feature | Why killed |
|---------|-----------|
| `gaal active` command | Process monitoring too fragile. Removed in v0.1.0. |
| `gaal show` command | Merged into `inspect`. Redundant commands removed. |
| SessionStatus enum | Status taxonomy was noise. Removed in v0.1.0. |
| --live, --watch, --active flags | Real-time monitoring features removed. |
| Velocity, context %, recent_errors fields | Heuristic calculations were unreliable. |
| Process blocks in output | No process monitoring in v0.1.0. |
| Stuck detection | Heuristic garbage. Wrong more than right. 50+ edge cases for near-zero value. |
| Parent-child linking | 1 out of 2,433 sessions ever linked. Dead feature. |
| Loop detection | Premature. Insufficient signal in JSONL to detect reliably. |

If you find yourself re-adding any of these: stop, re-read this section, and ask yourself why you think you'll succeed where 5+ attempts failed.

## Verification Protocol

**This is law. Every fix, every feature, every PR.**

### Before writing code

1. **Dump real data first.** `grep`, `jq`, `head` on actual JSONL files. See real field names, real structures, real edge cases.
2. **Never reason about what JSONL "should" contain.** Claude and Codex schemas are undocumented and change without notice. The only source of truth is the bytes on disk.
3. **Test your assumptions.** If you think a field is called `content`, grep for it. If you think events have a `type` field, prove it.

### While writing code

4. **One fix per commit.** No bundling. If fix A breaks, you can revert without losing fix B.
5. **Match code to reality, not reality to code.** If the JSONL has `arguments` in one place and `input` in another, handle both. Don't normalize upstream data you don't control.

### After writing code

6. **`cargo build --release` — always.** The installed binary is a symlink to `target/release/gaal`. Debug builds don't update it. If you run `cargo build` without `--release`, your fix doesn't take effect. This has burned us multiple times.
7. **Test against live sessions.** Run the built binary against real JSONL files on this machine. Not mocks, not synthetic data.
8. **Verify the output.** Don't assume "it compiled, so it works." Run the command, read the output, confirm it matches what the real data says.

### The evidence-first rule

When debugging: **dump first, code second.** The pattern that works:
```
grep real files → see actual field names → fix code to match reality → cargo build --release → test with real binary
```

The pattern that fails:
```
read Rust source → reason about what "should" work → write fix → cargo build (debug) → wonder why nothing changed
```

## Token Accounting (fixed 2026-03-28)

- **Cache tokens** are fully tracked: `cache_read_tokens` and `cache_creation_tokens` stored in DB, surfaced in `inspect --tokens`, and included in transcript frontmatter.
- **Peak context** = max(input_tokens + cache_read + cache_creation) across all turns. Represents actual API context window usage per turn. This is NOT file size.
- **Model-aware cost estimation:** `estimate_session_cost()` uses per-model pricing (Opus $15/$75, Sonnet $3/$15, Codex $2/$8 per Mtok). Cache read and creation tokens are priced separately.
- **Tool counting:** Both Claude and Codex tool uses are counted. Claude tools appear as `ContentBlock::ToolUse` inside `AssistantMessage` events. Codex tools appear as standalone `EventKind::ToolUse` events. Both paths increment `total_tools`.
- **Usage deduplication:** Claude uses `dedup_key` (message ID) to avoid double-counting. Codex uses cumulative `total_tokens` as dedup key to handle rate-limit bucket duplicates.
- **Error deduplication:** Error facts are keyed by `tool:{tool_use_id}` or `ts:{timestamp}|exit:{code}` to prevent double-counting.

## AX Error Philosophy

Gaal's errors are designed to teach calling agents. Every error includes:
1. **What went wrong** — specific, not generic
2. **A working example** — correct invocation the agent can copy
3. **A hint** — what to try next

Exit codes are meaningful and consistent: 0=success, 1=no results, 2=ambiguous ID, 3=not found, 10=no index, 11=parse error. The `-H` flag routes errors through `format_human()` for readable stderr output.

## Architecture Notes

- **Parser:** Dual Claude/Codex JSONL parsers. They have fundamentally different event schemas. Every feature touching parsed data must handle both.
- **DB:** SQLite for session metadata + Tantivy for full-text search. Use `savepoint_with_name()` for nested transactions — never `unchecked_transaction()`.
- **Detection:** Salt-based session discovery via content addressing.
- **Output:** JSON-first for agent consumption. Human-readable formatting via `--human` / `-H` flags.
- **Transcript rendering:** JSONL events are parsed into `SessionData`, then rendered to markdown with YAML frontmatter (session_id, date, model, tokens, cache breakdown).

## Key Paths

| Path | What |
|------|------|
| `src/commands/salt.rs` | Salt token generation for self-identification |
| `src/commands/find.rs` | JSONL file discovery by salt token (`find-salt` command) |
| `src/commands/inspect.rs` | Session detail view (merged `show` functionality) |
| `src/commands/ls.rs` | Session listing with envelope format |
| `src/commands/who.rs` | File/command activity search |
| `src/commands/handoff.rs` | LLM-powered handoff generation (`create-handoff`, supports `--jsonl` direct path) |
| `src/commands/index.rs` | Indexing pipeline |
| `src/commands/tag.rs` | Tag management with `tag ls` subcommand |
| `src/commands/transcript.rs` | Session transcript markdown generation |
| `src/db/schema.rs` | SQLite schema + autocommit guard + column migrations |
| `src/parser/` | Claude + Codex JSONL parsers |
| `src/parser/facts.rs` | Unified fact extraction from events (tool counting, error dedup, peak context) |
| `src/render/session_md.rs` | JSONL events to markdown renderer (frontmatter, turns, subagents) |
| `src/error.rs` | AX-compliant error types with `format_human()` method |
| `skill/SKILL.md` | Agent skill file for gaal |

## Build

```bash
cargo build --release
# Binary: target/release/gaal (symlinked to ~/.cargo/bin/gaal)
```

Clean build: ~8 min. Incremental: ~30s. Always use `--release`.

# CLAUDE.md — gaal

Session observability CLI for Claude Code and Codex. Rust, single binary, macOS-first.

## v0.1.0 Scope (public release)

Nine commands. Core session observability without monitoring features.

| Command | What it does |
|---------|-------------|
| `gaal inspect <id>` | Session detail view. Files, commands, timeline, git ops. |
| `gaal ls` | List sessions with envelope format and query_window. |
| `gaal who <verb> <target>` | Find sessions by file/command activity. Query_window support. |
| `gaal recall <topic>` | Ranked retrieval for session continuity. Handoff files + JSONL fallback. |
| `gaal search <query>` | Find sessions by content. BM25 ranked via Tantivy. |
| `gaal create-handoff <id>` | Generate handoff document via LLM extraction. |
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

## Architecture Notes

- **Parser:** Dual Claude/Codex JSONL parsers. They have fundamentally different event schemas. Every feature touching parsed data must handle both.
- **DB:** SQLite for session metadata + Tantivy for full-text search. Use `savepoint_with_name()` for nested transactions — never `unchecked_transaction()`.
- **Detection:** Salt-based session discovery via content addressing.
- **Output:** JSON-first for agent consumption. Human-readable formatting via `--human` / `-H` flags.

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
| `src/db/schema.rs` | SQLite schema + autocommit guard |
| `src/parser/` | Claude + Codex JSONL parsers |
| `ISSUES.md` | Full issue history (I1-I40+) |
| `TESTS.md` | Stress test harness |

## Build

```bash
cargo build --release
# Binary: target/release/gaal (symlinked to ~/.cargo/bin/gaal)
```

Clean build: ~8 min. Incremental: ~30s. Always use `--release`.

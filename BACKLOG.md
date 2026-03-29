# BACKLOG.md — gaal

## Shipped (v0.1.0)

| What | When | Session/Commit |
|------|------|---------------|
| Subagent P0 arc: `src/subagent/` module, DB indexing (5,170 subagents, 174 coordinators), `ls --include-subagents`, `inspect` Subagents table, `search` + `who` attribution, transcript DB-backed summaries | 2026-03-29 | [session: 2b0db33c] |
| AX sprint fixes: JSON error parity (hint/example fields), `create-handoff latest`, `find-salt` false success, `--session-type` filter on `ls`, exit code compliance | 2026-03-29 | [session: 66ce8874] |
| AX harness: 3-layer structure (layer1-errors, layer2-tasks, layer3-analysis), error quality scoring, first-attempt task workflows, trace analysis | 2026-03-29 | [session: 66ce8874] |
| Transcript fixes: XML stripping before truncation in `get_first_user_prompt()`, model-by-agent_id lookup, `ls` Task column | 2026-03-29 | [commit: 80db650] |
| DOCS.md creation + archive sweep: old root docs archived, README points to DOCS.md | 2026-03-29 | [commit: 46712e8] |
| docs/ folder restructure: monolithic DOCS.md replaced with 13-page docs/ folder, all links valid | 2026-03-29 | [session: this] |
| GAAL_HOME env var: allow override for relocatable data dir, enables sandboxed workers without HOME remapping | 2026-03-29 | [session: this] |
| CLAUDE.md rewrite with AX convention, verification protocol, feature kill list | 2026-03-29 | [commit: 1d2a70d] |
| BACKLOG.md reconciliation | 2026-03-29 | [commit: 1cc1b1d] |
| SKILL.md audit: verified against current command surface and binary behavior | 2026-03-29 | [commit: 3cd740a] |

---

## Open Backlog

| Priority | Item | Description |
|----------|------|-------------|
| P0 | Orphan recovery (4,051 files) | Parse `parentUuid` from subagent JSONL to reconstruct parent links for the 4,051 files pruned before `cleanupPeriodDays` was raised to 365. Fleet metadata (tokens, duration, status) is unrecoverable; facts (file_read/write/command) can be indexed. Most important remaining work. |
| P0 | SKILL.md rewrite | Philosophy-first rewrite. Kill eywa (~20% of current content), add vision/mission/design principles. Operational manual moves to reference/ material. Needs Opus 4.6 writer. |
| P1 | AX harness sandbox fix | Use `--sandbox none` for AX test workers (our own code, not untrusted). Fixes SQLite lockfile failures in Layer 2 tasks. Dispatch config issue, not a gaal code fix. Note: AX layer2 failures on salt/find-salt were caused by Codex sandboxing (SQLite lockfile + HOME remapping), not by the salt logic itself. Salt is reliable. |
| P1 | Subagent Phase 4 polish | Orphan handling, zero-turn subagents, Task column parent-description preference for v2.1.86+ sessions where `user_prompt` is not the task description. |
| P2 | Codex subagent audit | Verify Codex parser handles subagent JSONL correctly. Test coverage for Codex coordinator→subagent flows. No confirmed bugs yet — needs investigation. |
| P2 | `latest` selector in tag | `gaal tag latest add <tag>` — extend latest resolution beyond inspect/transcript to the tag command. |
| P2 | Agent-mux worker visibility | Workers dispatched via Bash have no `toolUseResult`, no subagent JSONL. Needs new metadata format from agent-mux side — not a gaal code problem until agent-mux emits it. |
| P3 | Incremental parsing | SHA-256 prefix trust layer on byte-offset resume. Prevents silent corruption when session files are rewritten from start. Framework exists (`parse_session_incremental()`); needs fingerprint computation + trust gate. |

---

## Killed

These were deliberately removed, not deferred. Do not re-add.

- ~~AX sandbox HOME lockfile as gaal code fix~~ — dispatch config issue, use `--sandbox none`
- ~~`gaal active` (process monitoring)~~ — too fragile, killed in v0.1.0 cut
- ~~Stuck/loop detection~~ — insufficient signal, wrong more than right
- ~~Parent-child linking via PID~~ — 1 out of 2,433 sessions ever linked; salt-based discovery replaced it

---

## Reference: Subagent Data Architecture

**Verified 2026-03-29** [session: 2b0db33c]

**Two-source model:**

| Source | Role | What it provides |
|--------|------|-----------------|
| Parent JSONL `toolUseResult` blocks | Fleet index | agentId, totalTokens, totalDurationMs, totalToolUseCount, status, prompt/description |
| Subagent JSONL (`subagents/agent-{agentId}.jsonl`) | Detail store | Full conversation, every tool call, every file read/write, per-turn token usage |

**Path determinism:** `Parent JSONL → toolUseResult.agentId → {session_dir}/subagents/agent-{agentId}.jsonl`

**Dead end — do not build on:** `SubagentProgress` events. Deprecated in CC v2.1.86+. Use only as legacy fallback for pre-v2.1.86 sessions.

**Target AX examples:**

`gaal who read src/render/session_md.rs` — attribution flows through parent to the subagent that did the work:
```
  7d5d03e4  2026-03-28  claude-opus-4-6     -> a59e6762 (Fix Agent rendering in transcripts)
```

`gaal inspect <parent-id>` — Subagents table sourced from parent `toolUseResult`, no subagent JSONL read needed:
```
  Subagents (34):
  ID        Model          Tokens    Duration  Description
  a59e6762  sonnet-4-6     75K       4m 47s    Fix Agent rendering in transcripts
```

`gaal inspect <subagent-id>` — same as any session, shows internal facts:
```
  Session: a59e6762 (subagent of 7d5d03e4)
  Files read: session_md.rs, CLAUDE.md
  Files written: session_md.rs
  Commands: cargo build --release
```

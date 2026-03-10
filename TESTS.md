# TESTS.md — Gaal Stress Tests

Tracked: 2026-03-10

---

## Test Matrix

| # | Name | Category | Difficulty | Status | Validates |
|---|------|----------|-----------|--------|-----------|
| 1 | Full Coordinator Workflow Chain | workflow | hard | PASS 8/8 | Pipeline integrity |
| 2 | Concurrent Index + Read Storm | concurrency | brutal | pending | I8 (db locked), WAL |
| 3 | Special Characters in Search | edge-case | hard | pending | I12 (parentheses) |
| 4 | Parent-Child Linking Integrity | cross-engine | hard | pending | I11 (linker) |
| 5 | Handoff Prefers Parent Over Child | edge-case | hard | pending | I10 (child-vs-parent) |
| 6 | Zero-Turn Session Pruning | data-integrity | hard | pending | I1 (noise filtering) |
| 7 | Index→Search Roundtrip | data-integrity | hard | pending | I4 (search consistency) |
| 8 | Scale: 500+ Sessions + Aggregates | scale | hard | pending | Performance, aggregates |
| 9 | Corrupt/Missing Data Degradation | failure-mode | brutal | pending | Parser robustness |
| 10 | Cross-Engine Mixed Recall + Who | cross-engine | hard | pending | Engine filtering, I14 |

---

## Test 1: Full Coordinator Workflow Chain

**Date:** 2026-03-10
**Runner:** Sonnet 4.6 subagent
**Result:** PASS 8/8

### Steps & Results

| Step | Command | Exit | Key Output | Result |
|------|---------|------|------------|--------|
| 1 | `gaal index backfill --force --since 7d` | 0 | `indexed: 0, skipped: 0, errors: 0` (already fresh) | PASS |
| 2 | `gaal ls --limit 1` | 0 | Session `dd6cebbb` (codex, gpt-5.3-codex, 19s, 15 tools) | PASS |
| 3 | `gaal show dd6cebbb` | 0 | Full detail: 15 commands (sed reads of Go source), 1 turn | PASS |
| 4 | `gaal show dd6cebbb --files write` | 0 | `written: [], edited: [], read: []` — read-only session, correct | PASS |
| 5 | `gaal who wrote "ISSUES.md" --since 30d` | 0 | 10 write events from session `2c47d1f0` | PASS |
| 6 | `gaal search "hardened" --limit 3` | 0 | 3 results from `fd40d96e`, score 16.75 | PASS |
| 7 | `gaal show dd6cebbb --tree` | 0 | Single root node, no children, completed | PASS |
| 8 | `gaal inspect dd6cebbb` | 0 | Context 25.8%/128K, 15 actions, velocity 3.0/min, no stuck | PASS |

### Notes
- Most recent session (`dd6cebbb`) was a short Codex read-only probe — no writes, no headline
- Steps 5-6 adapted to use substantive session `2c47d1f0` for file-write and search verification
- All commands returned valid JSON with correct semantics

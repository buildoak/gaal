<!-- Archived 2026-03-29: superseded by DOCS.md / BACKLOG.md -->
# ISSUES.md — Active Issues

Archived issues (v0.1.0 and earlier): _archive/ISSUES-v010-dormant.md

---

## I27: Automatic handoff generation

**Status:** Deferred
Generate handoffs automatically on session end, idle >30min, context >90%, or on resume. Prerequisites (I15/I16/I17/I18) are now moot (status removed, transactions fixed), so revisit feasibility.

---

## I46: Rethink `gaal inspect` design for agent consumption

**Status:** Deferred
Agents want a brief card (headline + metadata + transcript path) not a full dump. Should `inspect` default to summary card with `--full` gating full detail? Does inspect need to exist separately from `ls`?

---

## I47: `gaal who -H` results missing date ranges

**Status:** Deferred
Output shows which sessions touched a file but not when. Add `first_seen`/`last_seen` timestamps for matching facts to both JSON and `-H` output.

---

## I55: Fact extraction pipeline lossy for `create-handoff`

**Status:** Workaround shipped
`build_context()` misses conversational decisions (assistant replies not indexed as facts). Workaround: `create-handoff` now reads full session.md transcripts directly. Proper fix: index assistant turn decisions as facts.

---

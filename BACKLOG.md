# BACKLOG.md - Gaal Roadmap

This is the public roadmap for Gaal. It is not a private session ledger. Keep it
short, current, and useful to someone deciding whether to use or contribute to
the tool.

## Recently Shipped

| Area | Status |
| --- | --- |
| Multi-engine indexing | Codex, Claude Code, Antigravity CLI, Hermes Agent, and Gemini CLI are normalized into one session/fact model. |
| Deterministic transcripts | Per-session markdown transcripts can be rendered and read without loading raw JSONL by hand. |
| Activity slices | `gaal activity` renders historical, source-backed work slices across time windows. |
| Attribution | `gaal who` can answer which sessions read, wrote, ran, changed, touched, or deleted a target. |
| Short ID resolution | `gaal resolve` makes short session handles explicit and debuggable. |
| Subagent visibility | Coordinator/subagent relationships are indexed where source traces expose them. |
| Agent-facing skill | `skill/SKILL.md` documents the JSON-first agent workflow and safety model. |

## Open Roadmap

| Priority | Item | Why It Matters |
| --- | --- | --- |
| P1 | Public onboarding examples | New users should feel the "oh, this is useful" moment in the first five minutes. |
| P1 | AX harness sanitation | The harness is valuable, but generated outputs must stay local or be sanitized fixtures. |
| P1 | Hermes compatibility sweep | Hermes support works against known fixtures, but more installation shapes need coverage. |
| P2 | Session-level search reranking | Fact-level BM25 is good for exact evidence; session-level ranking may improve broad recall queries. |
| P2 | Activity performance pass | Rich activity slices can be expensive on very dense days; optimize after profiling release builds. |
| P2 | Gemini nested session discovery | Current Gemini support focuses on top-level sessions; nested/subagent-like sessions need fuller discovery. |
| P3 | Incremental parsing fingerprint guard | Byte-offset resume works; add a stronger rewrite/corruption guard before trusting offsets forever. |

## Deliberately Not Planned

These are out of scope unless new evidence changes the tradeoff.

| Item | Reason |
| --- | --- |
| Live process monitor | Gaal is a trace/index/read tool, not a daemon. |
| Stuck or loop detection | Too much heuristic noise. |
| Web UI | The current product center is the agent-facing CLI. |
| Secret redaction as a guarantee | Gaal reads private local traces; users must treat source logs and derived views as private data. |

## Contribution Shape

Good contributions usually include:

- one focused behavior change
- fixture or test coverage
- README/docs/skill updates when the public contract changes
- evidence from `cargo test` and `./tests/run-all.sh`

The high bar is simple: Gaal should make agent history easier to find and read
without pretending that generated summaries are the source of truth.

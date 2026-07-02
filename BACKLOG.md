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
| P1 | Seamless first installation flow | The agent-first install path should take a new user from repo link -> installed binary -> indexed traces -> first useful answer without guesswork. |
| P1 | Public onboarding examples | New users should feel the "oh, this is useful" moment in the first five minutes. Build 5-8 copy-paste prompts around work patterns, attribution, failed commands, topic search, activity checkout, and safe handoff preview. |
| P1 | Crates.io install path | Add `cargo install gaal` once the published crate is the supported public install path. Until then, source install remains canonical. |
| P1 | Installation guide polish | Keep README, `docs/getting-started.md`, and `skill/references/installation.md` aligned around one first-run path, zero-session recovery, `GAAL_HOME`, and LaunchAgent caveats. |
| P1 | Handoff onboarding section | Explain handoff dry-run review, provider support, side effects, warnings, and when generated continuity is appropriate. |
| P1 | AX harness sanitation | The harness is valuable, but generated outputs must stay local or be sanitized fixtures. |
| P1 | Hermes compatibility sweep | Hermes support works against known fixtures, but more installation shapes need coverage. |
| P2 | Higher-quality subagent workstream | Isolate subagent improvements from installation/onboarding work. Treat parent-child attribution, subagent transcript fidelity, and model/token accounting as their own quality track. |
| P2 | Session-level search reranking | Fact-level BM25 is good for exact evidence; session-level ranking may improve broad recall queries. |
| P2 | Activity performance pass | Rich activity slices can be expensive on very dense days; optimize after profiling release builds. |
| P2 | Gemini nested session discovery | Current Gemini support focuses on top-level sessions; nested/subagent-like sessions need fuller discovery. |
| P3 | Incremental parsing fingerprint guard | Byte-offset resume works; add a stronger rewrite/corruption guard before trusting offsets forever. |

## Pre-Release Checklist

Before the next public release:

- Verify source install from a fresh clone.
- Decide whether crates.io install is supported in this release.
- Run a fresh-agent install using only `skill/SKILL.md`.
- Confirm `README.md`, `docs/getting-started.md`, and
  `skill/references/installation.md` describe the same first-run flow.
- Add a zero-session recovery checklist for machines with no traces yet.
- Expand `skill/references/onboarding.md` into a real first-use prompt set.
- Test safe handoff preview from install docs without running non-dry-run
  generation.
- Run `cargo test`, `cargo check`, and the AX harness or a documented reduced
  AX gate.
- Confirm generated AX outputs are ignored and no private local trace material
  is tracked.

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

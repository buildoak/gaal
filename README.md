# gaal
Agent session observability CLI for AI coding agents. Like k9s, but for Claude Code and Codex sessions.

## Why it exists
AI coding sessions are black boxes. You run 5 Claude Code sessions in parallel, a few Codex workers, and by tomorrow you have no idea which session modified what file, which one burned $40 in a loop, or what you were even working on. Gaal makes all of that queryable -- a quiet nod to Gaal Dornick and the job of making sense of civilization-scale data.

## What it does
10 commands, grouped by purpose.

### Discovery
`ls` (fleet view), `active` (live processes), `show` (drill into session), `inspect` (health/stuck detection)

```bash
gaal ls --status active --engine claude -H
gaal inspect --active -H
```

```bash
gaal show latest --errors -H
gaal active --watch -H
```

### Search
`search` (BM25 full-text), `recall` (semantic retrieval, eywa replacement), `who` (inverted queries -- "who wrote this file?")

```bash
gaal search "retry loop sqlite lock" --since 14d -H
gaal who wrote "src/commands/index.rs" --since 30d -H
```

### Memory
`handoff` (LLM-powered handoff generation), `index` (backfill/maintain)

```bash
gaal index backfill
gaal handoff today --provider agent-mux -H
```

### Organization
`tag` (label sessions)

```bash
gaal tag f15a045c incident hotfix
gaal ls --tag incident -H
```

## Quick Start
1. Build
```bash
cargo build --release
```
2. Index: reads existing session JSONL, builds SQLite + Tantivy indexes
```bash
gaal index backfill
```
3. Try it
```bash
gaal ls -H
gaal recall "topic you worked on"
```

## Architecture
Gaal ingests raw session logs from Claude Code and Codex, extracts facts, and keeps the result queryable as both structured data and readable markdown. I built it so the machine does the indexing work and you stay in control of the timeline.

- Raw JSONL (written by Claude Code / Codex CLI) -> `gaal index backfill` -> SQLite index + Tantivy FTS + session markdown files
- Data lives at `~/.gaal/`
- LLM-powered operations (handoff generation) dispatch via `agent-mux`
- JSON output by default, `-H` for human-readable tables

## Numbers
- 2,393 sessions indexed (1,319 Claude Code, 1,074 Codex)
- 62,829 facts extracted
- 634 handoffs generated
- Sub-100ms recall

## License
MIT

Built by Nick Oak ([buildoak](https://github.com/buildoak)).

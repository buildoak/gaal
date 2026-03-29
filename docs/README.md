# gaal

`gaal` is session observability for AI coding agents. Claude Code and Codex both emit JSONL session logs, but those logs are usually 10-50MB blobs of undocumented, engine-specific event streams that are painful to inspect directly. `gaal` parses both formats, indexes them into SQLite plus Tantivy, and turns raw traces into answers in seconds. The core mental model is that sessions are first-class queryable artifacts, not throwaway logs.

## Quick Orientation

```bash
gaal ls -H
gaal inspect latest --tokens -H
gaal who wrote CLAUDE.md
gaal recall "auth refactor" --format brief
```

## Table of Contents

- [Getting Started](getting-started.md) -- build, install, first commands
- [Architecture](architecture.md) -- data model, two-source subagent model, session lifecycle
- [Commands](commands/) -- full command reference (one page per command group)
  - [Fleet View: ls](commands/fleet-view.md)
  - [Drill-Down: inspect, transcript](commands/drill-down.md)
  - [Attribution: who](commands/attribution.md)
  - [Search & Recall: search, recall](commands/search-recall.md)
  - [Handoff: create-handoff](commands/handoff.md)
  - [Self-Identification: salt, find-salt](commands/self-id.md)
  - [Index & Tags: index, tag](commands/index-tags.md)
- [Agent Guide](agent-guide.md) -- how agents should consume gaal output
- [Formats Reference](formats.md) -- output formats, exit codes
- [Eywa Migration](migration.md) -- eywa to gaal migration

## What gaal Does Not Do

- Real-time monitoring of active agent processes
- Stuck detection or session health heuristics
- Process-tree-based session discovery
- Loop detection over raw model behavior

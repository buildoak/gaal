# Gaal Philosophy

Gaal exists because raw agent logs are evidence, but they are almost never the
right interface for the next agent.

The source traces should stay intact. Around them, Gaal builds smaller faithful
views: indexed facts for discovery, deterministic markdown for reading, and
optional handoffs for continuity when compression is worth the cost.

## Core Principles

### Raw Traces Are Evidence

Claude Code, Codex, Grok Build, Antigravity CLI, Hermes Agent, and Gemini CLI own their
source artifacts. Gaal reads those artifacts and points back to them. A rendered
view can make evidence easier to use, but it should not pretend to replace the
evidence.

### Index Facts, Not Vibes

The index stores boring atoms: sessions, prompts, commands, file reads and
writes, errors, tags, metadata, and parent-child relationships. Those facts are
what let an agent move from a broad query to the exact session or file event
without loading a whole month of logs.

### Markdown Views Must Be Deterministic

Transcripts and activity slices are rendered from source-backed data. They are
not model-written interpretations. Their job is to be token-efficient and
readable while keeping the path back to raw evidence obvious.

### Compression Is Optional

Handoffs are useful when future work needs a compact brief. They are generated
notes, not ground truth. The normal path is discovery first, then a small
faithful view, then raw source only when precision demands it.

### Agents Are First-Class Users

Gaal defaults to JSON because agents need stable fields, exit codes, and
composable command output. Human tables exist for inspection, but the CLI should
remain scriptable and easy for agents to reason over.

### AX Testing Is Part Of The Product

Agent experience is not vibes either. The AX harness checks whether errors
teach, whether fresh agents can choose the right commands, and whether docs and
skill guidance actually route behavior. If agents keep making the same mistake,
that is product feedback.

### Local By Default, Private By Assumption

Core indexing, search, inspect, transcript rendering, attribution, tagging, and
recall over existing handoffs operate locally. But local traces can contain
prompts, source code, file paths, command output, and secrets accidentally
printed to a terminal. Treat both source traces and derived views as private
working data unless audited.

### Historical, Not Live

Gaal is not a daemon, process monitor, loop detector, status oracle, or secret
redaction guarantee. It is a traceability and navigation layer over what agent
harnesses have already written to disk.

## How This Shapes The CLI

The intended path is:

```text
broad query -> exact session -> deterministic view -> raw evidence if needed
```

That is why the command surface is split the way it is:

- `ls`, `search`, `who`, and `resolve` help agents discover where to look.
- `inspect`, `transcript`, and `activity` help agents read the smallest useful
  view.
- `recall` and `create-handoff` support continuity when a generated brief is
  useful.
- `index` and `tag` maintain derived local state without owning the source
  artifacts.

Navigate first. Spend tokens later.

# Getting Started

This page is the shortest path from a fresh clone to your first useful `gaal` query. If you already have local Claude Code, Codex, Gemini, agy/Antigravity, or Hermes session logs, you can usually get from zero to indexed sessions in under five minutes.

After any package-manager install, run:

```bash
gaal onboard
```

That command is designed for agentic installation. It points the installing
agent at the latest bundled skill/reference material and the first-launch
commands before any local traces are indexed.

## Build

You need a Rust toolchain installed locally.

Build with the release profile:

```bash
cargo build --release
```

Use `--release` for local development builds. For normal manual installation,
`cargo install --path . --force` refreshes the release binary in Cargo's bin
directory.

Rough build times:

- Clean build: about 8 minutes
- Incremental build: about 30 seconds

For the simplest local install, use the source installer from the checkout:

```bash
git clone https://github.com/buildoak/gaal.git
cd gaal
./install.sh --no-schedule
```

For manual installation without the helper:

```bash
cargo install --path . --force
```

Scheduled indexing is recommended after the first install, but it is explicit:

```bash
./install.sh --schedule
./install.sh print-plist
./install.sh status
```

The scheduled job runs only `gaal index backfill`. It does not create handoffs,
call LLM backends, or run recall.

## Requirements

`gaal` indexes session artifacts that already exist on disk. Before your first run, make sure you have:

- Local access to session logs under `~/.claude/projects/`, `~/.codex/`, `~/.gemini/tmp/`, `~/.gemini/antigravity-cli/brain/`, and/or `~/.hermes/state.db`
- A writable gaal home at `~/.gaal/`

If this is a brand-new machine with no agent sessions yet, `gaal index backfill` may create the index and still leave you with zero sessions. That is expected. Run an agent for a bit, then backfill again.

## First Index

Index your existing sessions:

```bash
gaal index backfill
```

If you also want rendered transcript markdown written during indexing:

```bash
gaal index backfill --with-markdown
```

After this completes, the database and full-text index under `~/.gaal/` are ready for queries.

## gaal Home

`gaal` stores all derived state under `~/.gaal/`:

```text
~/.gaal/
  index.db
  tantivy/
  config.toml
  prompts/
    handoff.md
  data/
    {engine}/
      sessions/YYYY/MM/DD/<id>.md
      handoffs/YYYY/MM/DD/<id>.md
```

What each part is for:

- `index.db`: SQLite store for indexed session metadata, facts, tags, and handoffs
- `tantivy/`: full-text search index used by `gaal search`
- `config.toml`: runtime configuration
- `prompts/handoff.md`: extraction prompt used for handoff generation
- `data/{engine}/sessions/YYYY/MM/DD/<id>.md`: rendered transcript markdown
- `data/{engine}/handoffs/YYYY/MM/DD/<id>.md`: generated handoff markdown

`{engine}` is usually `claude`, `codex`, `gemini`, `agy`, or `hermes`.

## Four Commands to Learn First

These four commands cover the main workflow: list sessions, inspect one deeply, attribute a change, and open the transcript.

```bash
gaal ls -H
gaal inspect latest --tokens -H
gaal who wrote AGENTS.md
gaal transcript latest
```

Example fleet view:

```text
$ gaal ls --limit 5 -H
ID        Engine  Started      Duration  Tokens       Peak  Tools  Model              CWD
--------  ------  -----------  --------  -----------  ----  -----  -----------------  -----------
acabe588  claude  today 18:25  3h 46m    2K / 13K     124K  74     claude-opus-4-6    coordinator
1ab21f89  claude  today 18:38  3h 25m    28 / 311     74K   8      claude-opus-4-6
65eeec4f  claude  today 19:03  2m 1s     2K / 495     69K   10     claude-sonnet-4-6
875f36ae  codex   today 21:50  8m 39s    180K / 21K   181K  114    gpt-5.4            gaal
```

Example token view:

```text
$ gaal inspect latest --tokens -H
Session: 875f36ae (codex, gpt-5.4)
Duration: 8m 39s
Tokens: input=180K output=21K cache_read=5.4M
Peak context: 181K
Estimated cost: $1.23
Tools used: 114
```

If you only remember the file or topic, `gaal who wrote ...` is usually the fastest path back to the right session. Once you have the session ID, `gaal inspect <id>` and `gaal transcript <id>` give you the detail.

## Config Defaults

These are the practical defaults in `~/.gaal/config.toml`:

| Key | Default |
|-----|---------|
| `llm.default_engine` | `codex` |
| `llm.default_model` | `gpt-5.4-mini` |
| `llm.timeout_secs` | `120` |
| `handoff.prompt` | `prompts/handoff.md` |
| `handoff.format` | `markdown` |
| `agent-mux.path` | `agent-mux` |
| `agent-mux.effort` | `xhigh` |

You do not need to change any of these to build, index, or run the first query. The usual first useful loop is:

```bash
cargo build --release
gaal index backfill
gaal ls -H
gaal inspect latest --tokens -H
```

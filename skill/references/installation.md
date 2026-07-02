# Installation Reference

Use this when a fresh agent or human needs to install Gaal from the public repo.

## Agent-First Path

Give the agent the public skill and ask it to install Gaal end to end:

```text
https://github.com/buildoak/gaal/blob/master/skill/SKILL.md
```

The agent should verify each step instead of assuming the binary or index
already exists.

## Manual Install

```bash
git clone https://github.com/buildoak/gaal.git
cd gaal
cargo install --path .
gaal --version
```

Then build the derived local index:

```bash
gaal index backfill
gaal index status
gaal ls -H --limit 5
```

If this is a new machine with no Claude Code, Codex, Gemini CLI, Antigravity
CLI, or Hermes traces yet, `index backfill` can succeed with zero sessions.
That is normal. Run an agent for a bit, then backfill again.

## Development Symlink

For development or scheduled indexing, a stable symlink can be better than a
copied `cargo install` binary:

```bash
cargo build --release
ln -sf "$PWD/target/release/gaal" /opt/homebrew/bin/gaal
```

The LaunchAgent example in `defaults/` expects `/opt/homebrew/bin/gaal`. Edit
the plist if your binary lives elsewhere.

## Sandboxes And CI

Use `GAAL_HOME` when the default `~/.gaal` is not writable or should stay
untouched:

```bash
export GAAL_HOME=/tmp/gaal-home
gaal index backfill
gaal ls --limit 5
```

`GAAL_HOME` relocates Gaal's derived database, Tantivy index, rendered
transcripts, config, and generated handoffs. It does not relocate source traces
written by the agent harnesses.

## Handoff Backend

Core indexing, search, inspect, transcript, attribution, tags, and recall over
existing handoffs do not require an LLM backend. `gaal create-handoff` is the
exception: real generation uses the configured provider and should be previewed
with `--dry-run`.

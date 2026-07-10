# Grok Playbook

Read this playbook when indexing, identifying, inspecting, handing off, or
troubleshooting native Grok Build sessions.

## Source and identity

- Treat `${GROK_HOME:-~/.grok}/sessions/<encoded-cwd>/<full-uuid>/` as one
  source artifact and keep the full UUID as its canonical session ID.
- Resolve the unique last eight dash-stripped UUID characters as a convenience
  alias. Use the full UUID when an alias is absent or ambiguous.
- Select `updates.jsonl` as the primary visible event source. Fall back to
  `chat_history.jsonl` only when updates contain no recoverable visible events;
  never merge the two streams.

## Indexing and visibility

- Run `gaal index backfill` for normal and scheduled indexing. Use
  `gaal index backfill --engine grok` only for a Grok-scoped diagnostic, repair,
  or test pass.
- Exclude thoughts, prompt context, system prompts, summaries/titles, rewind
  files, and non-text/image payload bodies from visible facts.
- Fail closed on unrecognized files and lifecycle records: omit their bodies
  and expose their counts through source diagnostics.
- Treat best-effort secret-pattern redaction and the 16,000-character retained
  preview for oversized tool output as defenses for derived text, not publication guarantees.
  Gaal never rewrites or sanitizes the raw Grok artifacts.

## Compatibility

- Run `gaal salt` and `gaal find-salt <literal-token> --engine grok` as separate
  calls. Salt matching accepts supported executed tool/action output, not user
  prompt echoes.
- Treat the legacy `jsonl_path` result and `create-handoff --jsonl` argument as
  source-artifact compatibility names; their Grok value is a session directory.
- Preview `gaal create-handoff <full-uuid-or-alias> --dry-run` before generating
  a handoff. Handoff generation remains opt-in and may call an external model.

## Verify and troubleshoot

```bash
gaal index backfill --engine grok
gaal index status
gaal ls --engine grok -H
gaal resolve <full-uuid-or-last8> --engine grok -H
gaal inspect <full-uuid-or-last8> --source -H
gaal transcript <full-uuid-or-last8>
gaal index reindex <full-uuid-or-last8>
```

- Inspect `index status` for Grok source-artifact, unknown, malformed, private,
  and redacted counters. Inspect one session with `--source` to see the selected
  artifact and parser observations without exposing excluded bodies.
- If lookup fails, resolve the full UUID first; a last-eight alias is registered
  only when unique. If discovery finds nothing, confirm `GROK_HOME` and the
  expected `sessions/<encoded-cwd>/<full-uuid>/` layout before forcing reindex.
- If parser counters rise after a Grok update, preserve the raw source, inspect
  its shape privately, add a sanitized fixture, and update parser tests before
  accepting the new record type.

## Current boundary

Treat Grok support as native coding-session observability: discovery, indexing,
search, attribution, transcripts, self-identification, and optional handoffs.
Do not infer support for every Grok platform semantic such as forks, exports,
subagents, memory, MCP/plugins, or ACP unless the public contract and fixtures
explicitly add it.

# `gaal search`

Purpose: full-text search over indexed facts using Tantivy.

## Usage

```bash
gaal search [OPTIONS] [QUERY]
```

## Flags

- `--since <duration|date>`: default `30d`
- `--cwd <substring>`
- `--engine <claude|codex>`
- `--field <prompts|replies|commands|errors|files|all>`: default `all`
- `--context <n>`: default `2`
- `--limit <n>`: default `20`
- `-H`, `--human`

## JSON Output

Default output is an envelope with:

- `query_window`
- `shown`
- `total`
- `results`

Each result includes `session_id`, `engine`, `turn`, `fact_type`, `subject`, `snippet`, `ts`, `score`, `session_headline`, `session_type`, and optional `parent_id`.

## Real Example

```bash
$ gaal search subagent --limit 2
{
  "query_window": {
    "from": "2026-02-27",
    "to": "2026-03-29"
  },
  "shown": 2,
  "total": 13,
  "results": [
    {
      "session_id": "aea2ddc4",
      "engine": "claude",
      "turn": 29,
      "fact_type": "command",
      "subject": "grep -rn \"20\\b\\|subagent.*limit\\|table.*cap\\|MAX.*SUBAGENT\\|SUBAGENT.*MAX\\|top.*subagent\\|subagent.*",
      "snippet": "grep -rn \"20\\b\\|subagent.*limit\\|table.*cap\\|MAX.*SUBAGENT\\|SUBAGENT.*MAX\\|top.*subagent\\|subagent.*top\" /Users/otonashi/thinking/building/gaal/src/ --include=\"*.rs\" | grep -v \"target\" | head -20",
      "ts": "2026-03-29T05:29:59.567Z",
      "score": 15.346081,
      "session_headline": "",
      "session_type": "subagent",
      "parent_id": "2b0db33c"
    }
  ]
}
```

# `gaal recall`

Purpose: ranked continuity retrieval over generated handoffs.

## Usage

```bash
gaal recall [OPTIONS] [QUERY]
```

## Flags

- `--days-back <n>`: default `14`
- `--limit <n>`: default `3`
- `--format <summary|handoff|brief|full|eywa>`: default `brief`
- `--substance <n>`: default `1`
- `-H`, `--human`

## Output Formats

- `brief`: compact session summary blocks.
- `summary`: structured fields only.
- `handoff`: structured summary plus raw handoff content.
- `full`: summary plus handoff, files, and errors.
- `eywa`: legacy markdown-oriented layout.

If no query is passed, `recall` prints usage help and exits successfully.

## Real Example

```bash
$ gaal recall subagent --limit 2 -H
━━━ Session 2b0db33c (2026-03-29) ━━━
Headline: Refined gaal’s subagent architecture, shipped the first working subagent engine, and closed the main AX gaps around who, ls, inspect, and transcript rendering.
Projects: gaal, coordinator
Keywords: gaal, subagent, transcript, who, BACKLOG.md
Substance: 3 | Duration: 327m | Score: 44.9
```

## Related Commands

- [`gaal who`](./attribution.md)
- [`gaal create-handoff`](./handoff.md)

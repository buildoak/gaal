# `gaal who`

Purpose: inverted attribution query. Ask which sessions read, wrote, ran, touched, changed, or deleted something.

## Usage

```bash
gaal who [OPTIONS] [VERB] [TARGET]
```

## Verbs

- `read`
- `wrote`
- `ran`
- `touched`
- `changed`
- `deleted`

## Flags

- `--since <duration|date>`: default `7d`
- `--before <date|timestamp>`
- `--cwd <substring>`
- `--engine <claude|codex|gemini>`
- `--tag <tag>`
- `--failed`: for `ran`, only non-zero command exits
- `--limit <n>`: default `10`
- `-F`, `--full`: return per-fact rows instead of grouped sessions
- `-H`, `--human`

## JSON Output

Default grouped mode returns matches by session with:

- `session_id`
- `engine`
- `latest_ts`
- `fact_count`
- `subjects`
- `headline`
- `session_type`
- optional `parent_id`

`--full` returns one row per matched fact with `fact_type`, `subject`, `detail`, and timestamp.

## Human Output

Human mode prints a brief session table by default. For subagent rows, parent-to-subagent attribution is shown inline.

## Real Example

```bash
$ gaal who wrote CLAUDE.md --limit 2
{
  "query_window": {
    "from": "2026-03-22",
    "to": "2026-03-29"
  },
  "shown": 2,
  "total": 4,
  "sessions": [
    {
      "session_id": "a2608f02",
      "engine": "claude",
      "latest_ts": "2026-03-28T18:25:28.470Z",
      "fact_count": 2,
      "subjects": ["CLAUDE.md"],
      "headline": null,
      "session_type": "subagent",
      "parent_id": "7d5d03e4"
    }
  ]
}
```

## Related Commands

- [`gaal inspect`](./drill-down.md)
- [`gaal search`](./search-recall.md)

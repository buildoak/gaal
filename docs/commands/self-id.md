# `gaal salt`

Purpose: generate a unique salt token for content-addressed self-identification.

## Usage

```bash
gaal salt
```

## Output Format

Raw token string on stdout, not JSON.

Format:

```text
GAAL_SALT_<16 hex chars>
```

## Real Example

```bash
$ gaal salt
GAAL_SALT_d0a6e1d5530bf6c9
```

# `gaal find-salt`

Purpose: scan Claude and Codex JSONL trees and return the first file containing a salt token.

## Usage

```bash
gaal find-salt [OPTIONS] [SALT]
```

## Flags

- `-H`, `--human`

## JSON Output

- `session_id`
- `engine`
- `jsonl_path`

Notes:

- The returned `session_id` is derived from the JSONL filename stem, so Codex and Claude shapes differ.
- This command scans `~/.claude/projects/` and `~/.codex/`.

## Real Example

```bash
$ gaal find-salt GAAL_SALT_d0a6e1d5530bf6c9
{"engine":"codex","jsonl_path":"/Users/otonashi/.codex/sessions/2026/03/29/rollout-2026-03-29T14-46-13-019d3933-90c8-7cc3-b974-a910ab3f2e83.jsonl","session_id":"rollout-2026-03-29T14-46-13-019d3933-90c8-7cc3-b974-a910ab3f2e83"}
```

## Self-Handoff Protocol

1. Run `gaal salt` and capture the emitted token.
2. Echo that token into the live session so it is flushed into the session JSONL.
3. Run `gaal find-salt <token>` to resolve the JSONL path, then `gaal create-handoff --jsonl <path>`.

These must be separate tool calls because `salt` output has to be written into the session log before `find-salt` scans for it. If `find-salt` runs before the tool result is flushed, discovery can miss the active session.

## Related Commands

- [`gaal create-handoff`](./handoff.md)
- [`gaal transcript`](./drill-down.md)

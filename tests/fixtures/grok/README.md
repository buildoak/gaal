# Grok Fixtures

Synthetic Grok Build CLI 0.2.93-style fixtures for native Gaal indexing.

These fixtures model the observed local layout:

```text
${GROK_HOME}/sessions/<percent-encoded-cwd>/<full-uuid>/
  summary.json
  updates.jsonl
  chat_history.jsonl
  events.jsonl
  prompt_context.json
  signals.json
  system_prompt.txt
  rewind_points.jsonl
```

Rules:

- All message content is synthetic.
- Session IDs are full UUIDs; two fixtures intentionally share the same first
  eight characters to guard against prefix truncation.
- `updates.jsonl` is the primary source when non-empty and recoverable.
- `chat_history.jsonl` is fallback only when `updates.jsonl` is absent, empty,
  malformed-only, or has no recoverable visible events.
- Parsers must ignore unknown update kinds.
- Parsers must omit `agent_thought_chunk`, `reasoning`, system records,
  encrypted content, generated titles, summaries, prompt context, resources,
  rewind bodies, and large/raw tool input-output bodies by default.
- AgentMUX files, if present in later fixtures, are poison inputs only and must
  not be required for discovery.

Visible canaries expected to index:

- `GROK_VISIBLE_USER_PROMPT_SHOULD_INDEX`
- `GROK_VISIBLE_ASSISTANT_REPLY_SHOULD_INDEX`
- `GROK_VISIBLE_TOOL_RESULT_SHOULD_INDEX`
- `[REDACTED_TELEGRAM_BOT_TOKEN]` from a fake visible token canary

Private canaries expected never to index:

- `1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi`
- `GROK_PRIV_CONFIG_SHOULD_NOT_INDEX`
- `GROK_PRIV_SYSTEM_PROMPT_SHOULD_NOT_INDEX`
- `GROK_PRIV_PROMPT_CONTEXT_SHOULD_NOT_INDEX`
- `GROK_PRIV_REWIND_BODY_SHOULD_NOT_INDEX`
- `GROK_PRIV_THOUGHT_CHUNK_SHOULD_NOT_INDEX`
- `GROK_PRIV_RAWINPUT_BODY_SHOULD_NOT_INDEX`
- `GROK_PRIV_RAWOUTPUT_BODY_SHOULD_NOT_INDEX`
- `GROK_PRIV_SUMMARY_SHOULD_NOT_INDEX`
- `GROK_PRIV_TITLE_SHOULD_NOT_INDEX`

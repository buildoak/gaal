# Agy Fixtures

This fixture tree is a sanitized fake `HOME` for Gaal agy integration tests.
It intentionally contains no real Antigravity transcript content.

Layout covered:

- `.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript_full.jsonl`
- `.gemini/antigravity-cli/brain/<uuid>/.system_generated/logs/transcript.jsonl`
- `.gemini/antigravity-cli/cache/last_conversations.json`

The UUIDs are stable so tests can assert Gaal's 8-character session IDs:

- `12345678-90ab-cdef-1234-567890abcdef` -> `12345678`
- `fedcba09-8765-4321-fedc-ba0987654321` -> `fedcba09`

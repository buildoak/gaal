# Hermes Fixture

This fixture is synthetic. It preserves the table shape and message/tool-call
patterns observed in Hermes Agent `state.db` without copying private content,
paths, tokens, account IDs, or real session text.

Use `state.sql` to create a temporary SQLite database for tests.

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    user_id TEXT,
    model TEXT,
    model_config TEXT,
    system_prompt TEXT,
    parent_session_id TEXT REFERENCES sessions(id),
    started_at REAL NOT NULL,
    ended_at REAL,
    end_reason TEXT,
    message_count INTEGER DEFAULT 0,
    tool_call_count INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    billing_provider TEXT,
    billing_base_url TEXT,
    billing_mode TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL,
    cost_status TEXT,
    cost_source TEXT,
    pricing_version TEXT,
    title TEXT
);

CREATE INDEX idx_sessions_source ON sessions(source);
CREATE INDEX idx_sessions_parent ON sessions(parent_session_id);
CREATE INDEX idx_sessions_started_desc ON sessions(started_at DESC);
CREATE UNIQUE INDEX idx_sessions_title_unique ON sessions(title) WHERE title IS NOT NULL;

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    content TEXT,
    tool_call_id TEXT,
    tool_calls TEXT,
    tool_name TEXT,
    timestamp REAL NOT NULL,
    token_count INTEGER,
    finish_reason TEXT,
    reasoning TEXT,
    reasoning_details TEXT,
    codex_reasoning_items TEXT
);

CREATE INDEX idx_messages_session_ts ON messages(session_id, timestamp);
CREATE VIRTUAL TABLE messages_fts USING fts5(content, content='messages', content_rowid='id');

INSERT INTO sessions (
    id, source, model, parent_session_id, started_at, ended_at, end_reason,
    message_count, tool_call_count, input_tokens, output_tokens,
    cache_read_tokens, cache_write_tokens, reasoning_tokens, title
) VALUES
('20260504_101010_a1b2c3', 'cli', 'hermes-test-model', NULL, 1777903810.125, 1777903870.500, 'completed', 3, 1, 120, 45, 10, 5, 7, 'Synthetic CLI fixture'),
('20260504_111111_d4e5f6', 'telegram', 'hermes-test-model', NULL, 1777907471.000, 1777907500.000, 'completed', 2, 0, 80, 32, 0, 0, 0, 'Synthetic Telegram fixture'),
('cron_fixture_20260504_121212', 'cron', 'hermes-test-model', NULL, 1777911132.000, 1777911160.000, 'completed', 1, 0, 30, 12, 0, 0, 0, 'Synthetic Cron fixture'),
('20260504_131313_parentaa', 'cli', 'hermes-test-model', NULL, 1777914793.000, 1777914850.000, 'completed', 1, 0, 40, 20, 0, 0, 0, 'Synthetic Parent fixture'),
('20260504_141414_childbb', 'cli', 'hermes-test-model', '20260504_131313_parentaa', 1777918454.000, 1777918520.000, 'completed', 2, 0, 60, 25, 0, 0, 0, 'Synthetic Continuation fixture');

INSERT INTO messages (
    session_id, role, content, tool_call_id, tool_calls, tool_name, timestamp,
    token_count, finish_reason, reasoning, reasoning_details, codex_reasoning_items
) VALUES
('20260504_101010_a1b2c3', 'user', 'Please inspect hermes_fixture_alpha and report the synthetic status.', NULL, NULL, NULL, 1777903811.000, 18, NULL, NULL, NULL, NULL),
('20260504_101010_a1b2c3', 'assistant', 'I will run a deterministic terminal check for hermes_fixture_alpha.', NULL, '[{"id":"call_terminal_1","type":"function_call","call_id":"call_terminal_1","response_item_id":"resp_cli_1","function":{"name":"terminal","arguments":"{\"command\":\"printf hermes_fixture_alpha\",\"workdir\":\"/fixture/hermes\"}"}}]', NULL, 1777903820.000, 24, NULL, NULL, NULL, NULL),
('20260504_101010_a1b2c3', 'tool', 'hermes_fixture_alpha', 'call_terminal_1', NULL, 'terminal', 1777903821.000, 5, NULL, NULL, NULL, NULL),
('20260504_111111_d4e5f6', 'user', 'Telegram synthetic prompt containing hermes_fixture_beta.', NULL, NULL, NULL, 1777907472.000, 10, NULL, NULL, NULL, NULL),
('20260504_111111_d4e5f6', 'assistant', 'Telegram synthetic answer for hermes_fixture_beta.', NULL, NULL, NULL, 1777907479.000, 10, 'stop', NULL, NULL, NULL),
('cron_fixture_20260504_121212', 'session_meta', '{"trigger":"cron","fixture":"hermes_fixture_cron"}', NULL, NULL, NULL, 1777911133.000, 7, NULL, NULL, NULL, NULL),
('20260504_131313_parentaa', 'user', 'Parent session establishes hermes_fixture_parent.', NULL, NULL, NULL, 1777914795.000, 9, NULL, NULL, NULL, NULL),
('20260504_141414_childbb', 'user', 'Continuation session resumes hermes_fixture_child.', NULL, NULL, NULL, 1777918455.000, 9, NULL, NULL, NULL, NULL),
('20260504_141414_childbb', 'assistant', 'Continuation answer for hermes_fixture_child.', NULL, NULL, NULL, 1777918465.000, 9, 'stop', NULL, NULL, NULL);

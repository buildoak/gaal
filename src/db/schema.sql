CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    engine TEXT NOT NULL CHECK(engine IN ('claude', 'codex')),
    model TEXT,
    cwd TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    exit_signal TEXT,
    last_event_at TEXT,
    parent_id TEXT REFERENCES sessions(id),
    session_type TEXT DEFAULT 'standalone' CHECK(session_type IN ('standalone', 'coordinator', 'subagent')),
    jsonl_path TEXT NOT NULL,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_creation_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    total_tools INTEGER DEFAULT 0,
    total_turns INTEGER DEFAULT 0,
    peak_context INTEGER DEFAULT 0,
    last_indexed_offset INTEGER DEFAULT 0,
    subagent_type TEXT
);

CREATE TABLE IF NOT EXISTS facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    ts TEXT NOT NULL,
    turn_number INTEGER,
    fact_type TEXT NOT NULL CHECK(fact_type IN (
        'file_read', 'file_write', 'command', 'error',
        'git_op', 'user_prompt', 'assistant_reply', 'task_spawn'
    )),
    subject TEXT,
    detail TEXT,
    exit_code INTEGER,
    success INTEGER
);

CREATE TABLE IF NOT EXISTS handoffs (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id),
    headline TEXT,
    projects TEXT,
    keywords TEXT,
    substance INTEGER DEFAULT 0,
    duration_minutes INTEGER DEFAULT 0,
    generated_at TEXT,
    generated_by TEXT,
    content_path TEXT
);

CREATE TABLE IF NOT EXISTS session_tags (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    tag TEXT NOT NULL,
    PRIMARY KEY (session_id, tag)
);

CREATE INDEX IF NOT EXISTS idx_facts_session_ts ON facts(session_id, ts);
CREATE INDEX IF NOT EXISTS idx_facts_type_ts ON facts(fact_type, ts);
CREATE INDEX IF NOT EXISTS idx_facts_subject ON facts(subject);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_id);
CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at);
CREATE INDEX IF NOT EXISTS idx_sessions_cwd ON sessions(cwd);
CREATE INDEX IF NOT EXISTS idx_sessions_engine ON sessions(engine);
CREATE INDEX IF NOT EXISTS idx_sessions_type ON sessions(session_type);
CREATE INDEX IF NOT EXISTS idx_sessions_subagent_type ON sessions(subagent_type);
CREATE INDEX IF NOT EXISTS idx_handoffs_substance ON handoffs(substance);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON session_tags(tag);

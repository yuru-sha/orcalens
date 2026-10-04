CREATE TABLE sessions_new (
    id INTEGER PRIMARY KEY,
    run_id INTEGER REFERENCES runs(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_session_id TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    UNIQUE (provider, provider_session_id)
) STRICT;

CREATE TABLE tool_calls_new (
    id INTEGER PRIMARY KEY,
    run_id INTEGER REFERENCES runs(id) ON DELETE CASCADE,
    session_id INTEGER REFERENCES sessions_new(id) ON DELETE SET NULL,
    call_id TEXT,
    tool_name TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT,
    input_hash TEXT,
    output_hash TEXT,
    start_event_id INTEGER REFERENCES raw_events(id) ON DELETE SET NULL,
    end_event_id INTEGER REFERENCES raw_events(id) ON DELETE SET NULL,
    normalization_key TEXT UNIQUE,
    turn_item_id TEXT,
    start_epoch TEXT,
    start_seq INTEGER
) STRICT;

CREATE TABLE skill_calls_new (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    session_id INTEGER REFERENCES sessions_new(id) ON DELETE SET NULL,
    skill_name TEXT NOT NULL,
    source TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT,
    arguments_hash TEXT
) STRICT;

INSERT INTO sessions_new (id, run_id, provider, provider_session_id, started_at, ended_at)
SELECT id, run_id, provider, provider_session_id, started_at, ended_at FROM sessions;

INSERT INTO tool_calls_new (id, run_id, session_id, call_id, tool_name, started_at, ended_at, status, input_hash, output_hash)
SELECT id, run_id, session_id, call_id, tool_name, started_at, ended_at, status, input_hash, output_hash FROM tool_calls;

INSERT INTO skill_calls_new (id, run_id, session_id, skill_name, source, started_at, ended_at, status, arguments_hash)
SELECT id, run_id, session_id, skill_name, source, started_at, ended_at, status, arguments_hash FROM skill_calls;

DROP TABLE tool_calls;
DROP TABLE skill_calls;
DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;
ALTER TABLE tool_calls_new RENAME TO tool_calls;
ALTER TABLE skill_calls_new RENAME TO skill_calls;

CREATE INDEX sessions_run_id ON sessions(run_id);
CREATE INDEX tool_calls_name_started_at ON tool_calls(tool_name, started_at);
CREATE INDEX tool_calls_session_call_id ON tool_calls(session_id, call_id);
CREATE INDEX tool_calls_session_turn_item_id ON tool_calls(session_id, turn_item_id);
CREATE INDEX skill_calls_name_started_at ON skill_calls(skill_name, started_at);

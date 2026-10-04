ALTER TABLE runs ADD COLUMN normalization_key TEXT;
ALTER TABLE runs ADD COLUMN turn_item_id TEXT;
ALTER TABLE runs ADD COLUMN start_event_id INTEGER REFERENCES raw_events(id) ON DELETE SET NULL;
ALTER TABLE runs ADD COLUMN end_event_id INTEGER REFERENCES raw_events(id) ON DELETE SET NULL;
CREATE UNIQUE INDEX runs_normalization_key ON runs(normalization_key);
ALTER TABLE tool_calls ADD COLUMN start_index INTEGER;

CREATE TABLE run_sessions (
    run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    source_event_id INTEGER REFERENCES raw_events(id) ON DELETE SET NULL,
    PRIMARY KEY (run_id, session_id)
) STRICT;

CREATE INDEX run_sessions_session_id ON run_sessions(session_id, run_id);
INSERT INTO run_sessions (run_id, session_id)
SELECT run_id, id FROM sessions WHERE run_id IS NOT NULL;

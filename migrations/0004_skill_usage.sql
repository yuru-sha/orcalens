CREATE TABLE skill_inventory_new (
    id INTEGER PRIMARY KEY,
    skill_name TEXT NOT NULL,
    source TEXT NOT NULL,
    path TEXT NOT NULL,
    UNIQUE (skill_name, source, path)
) STRICT;
INSERT INTO skill_inventory_new (skill_name, source, path)
SELECT skill_name, source, CAST(id AS TEXT) FROM skill_inventory;
DROP TABLE skill_inventory;
ALTER TABLE skill_inventory_new RENAME TO skill_inventory;

CREATE TABLE skill_calls_new (
    id INTEGER PRIMARY KEY,
    run_id INTEGER REFERENCES runs(id) ON DELETE CASCADE,
    session_id INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
    skill_name TEXT NOT NULL,
    source TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT,
    arguments_hash TEXT,
    source_event_id INTEGER REFERENCES raw_events(id) ON DELETE SET NULL,
    dedup_key TEXT NOT NULL UNIQUE
) STRICT;
INSERT INTO skill_calls_new (id, run_id, session_id, skill_name, source, started_at, ended_at, status, arguments_hash, dedup_key)
SELECT id, run_id, session_id, skill_name, source, started_at, ended_at, status, arguments_hash, 'legacy:' || id FROM skill_calls;
DROP TABLE skill_calls;
ALTER TABLE skill_calls_new RENAME TO skill_calls;
CREATE INDEX skill_calls_name_started_at ON skill_calls(skill_name, started_at);

CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    source_type TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_identity TEXT NOT NULL,
    UNIQUE (source_type, source_path, source_identity)
) STRICT;

CREATE TABLE scan_checkpoints (
    source_id INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    cursor TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE TABLE raw_events (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    source_identity TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    source_timestamp TEXT,
    payload TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    UNIQUE (source_id, source_identity)
) STRICT;

CREATE TABLE repositories (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    display_name TEXT
) STRICT;

CREATE TABLE workspaces (
    id INTEGER PRIMARY KEY,
    repository_id INTEGER REFERENCES repositories(id) ON DELETE SET NULL,
    path TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    source_key TEXT NOT NULL,
    repository_id INTEGER REFERENCES repositories(id) ON DELETE SET NULL,
    title TEXT,
    UNIQUE (source, source_key)
) STRICT;

CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
    workspace_id INTEGER REFERENCES workspaces(id) ON DELETE SET NULL,
    started_at TEXT,
    ended_at TEXT,
    outcome TEXT,
    agent TEXT,
    model TEXT
) STRICT;

CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_session_id TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    UNIQUE (provider, provider_session_id)
) STRICT;

CREATE TABLE tool_calls (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    session_id INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
    call_id TEXT,
    tool_name TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT,
    input_hash TEXT,
    output_hash TEXT
) STRICT;

CREATE TABLE skill_inventory (
    id INTEGER PRIMARY KEY,
    skill_name TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL
) STRICT;

CREATE TABLE skill_calls (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    session_id INTEGER REFERENCES sessions(id) ON DELETE SET NULL,
    skill_name TEXT NOT NULL,
    source TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    status TEXT,
    arguments_hash TEXT
) STRICT;

CREATE TABLE findings (
    id INTEGER PRIMARY KEY,
    run_id INTEGER REFERENCES runs(id) ON DELETE CASCADE,
    finding_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    details TEXT NOT NULL
) STRICT;

CREATE INDEX raw_events_observed_at ON raw_events(observed_at);
CREATE INDEX runs_started_at ON runs(started_at);
CREATE INDEX sessions_run_id ON sessions(run_id);
CREATE INDEX tool_calls_name_started_at ON tool_calls(tool_name, started_at);
CREATE INDEX skill_calls_name_started_at ON skill_calls(skill_name, started_at);
CREATE INDEX findings_type_severity ON findings(finding_type, severity);

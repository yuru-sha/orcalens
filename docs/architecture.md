# Architecture

## Design principles

1. **Read-only source integration**
   - Never mutate Orca's journal or provider transcripts.
   - Open source SQLite databases read-only where possible.

2. **Normalize before analyzing**
   - Provider- and Orca-specific formats terminate at adapters.
   - Analysis operates on stable orcalens-owned types.

3. **Keep raw evidence**
   - Preserve enough raw source material or hashes to re-run parsers and explain findings.

4. **Deterministic findings first**
   - Start with counts, durations, failures, repetitions, and recency.
   - Add heuristic/AI recommendations only after the underlying metrics are trustworthy.

5. **Task != Run != Session**
   - A task may have multiple runs.
   - A run may use one or more sessions.
   - A session belongs to a provider runtime and can emit many tool/skill events.

## Current implementation

The Rust crate exposes `scan`, `runs`, `skills`, `waste`, and `report` through a typed command enum. `reporting` owns the shared output contract. JSON output is an object with `command` and `data` fields. The list commands return empty `items` arrays. `scan` imports new Orca journal rows into `raw_events`. `report` reads row counts from the local database.

`storage` opens the orcalens-owned database at `$HOME/.local/share/orcalens/orcalens.db`, or at the path in `ORCALENS_DB`. It enables SQLite foreign keys and applies embedded migrations in version order. `schema_migrations` records applied versions. Each migration and its ledger entry commit in one transaction.

On Unix, `storage` restricts the analytics database file to owner read and write. Raw Orca conversation data must not be readable by other local users.

`collectors` discovers and reads Orca journals without invoking Orca's reducer. `normalizers` and `analyzers` remain empty. Provider transcripts are not collected. A dashboard is out of scope.

## Core entities

### Task

Logical unit of work, such as a GitHub issue or an Orca task.

Suggested fields:

- id
- source
- source_key
- repository_id
- title

### Run

One attempt to execute a task.

Suggested fields:

- id
- task_id
- workspace_id
- started_at
- ended_at
- outcome
- agent
- model

### Session

Provider session observed by Orca.

Suggested fields:

- id
- run_id
- provider
- provider_session_id
- started_at
- ended_at

### ToolCall

- id
- run_id
- session_id
- call_id
- tool_name
- started_at
- ended_at
- status
- input_hash
- output_hash

### SkillCall

- id
- run_id
- session_id
- skill_name
- source
- started_at
- ended_at
- status
- arguments_hash

### RawEvent

- id
- source_type
- source_path
- source_identity
- observed_at
- source_timestamp
- payload
- payload_hash

## Orca journal adapter

Orca stores its host-level journal in `agent-session-journal.db`. The collector searches directories from `ORCA_STATE_DIRS` and `ORCA_STATE_DIR`, followed by `$HOME/.orca` and `$HOME/.local/share/orca`.

The current published database schema uses `PRAGMA user_version` 3 or 4. The collector requires `journal_sessions(session_id, epoch)` and `journal_rows(session_id, epoch, seq, ts, row_json)`. Epochs are text identifiers. Timestamps are integer milliseconds. `journal_sessions.epoch` identifies the published epoch, and rows from other epochs are ignored.

The adapter opens the database read-only, validates the schema before writing destination state, and imports row JSON unchanged into `raw_events`. The checkpoint stores the last sequence for each session and epoch. A changed epoch starts a new cursor while preserving earlier raw events. Unsupported schema versions and missing required fields fail with a diagnostic.

## Skill invocation detection

Skill detection must be adapter-based because providers can represent skill invocation differently.

Observed Orca native-chat tests include surfaced skill envelopes such as:

`<command-name>/plugin:skill-name</command-name>`

This is enough to justify an MVP detector, but it must not be treated as the only possible representation.

Proposed interface:

```text
SkillInvocationDetector
  detect(event/transcript item) -> zero or more SkillInvocation
```

Initial implementations:

- CodexSkillDetector
- OmpSkillDetector
- ClaudeSkillDetector
- OpenCodeSkillDetector

## Storage

Use an orcalens-owned SQLite database.

Initial tables:

- schema_migrations
- sources
- scan_checkpoints
- raw_events
- repositories
- workspaces
- tasks
- runs
- sessions
- tool_calls
- skill_inventory
- skill_calls
- findings

Indexes should favor:

- timestamp/range scans
- session -> run lookups
- skill_name + timestamp
- tool_name + timestamp
- finding type/severity

## Incremental scanning

The current `scan` command has no collectors and imports no data. When collectors are added, each scan must be idempotent.

Each source adapter should maintain a checkpoint based on the strongest stable cursor available, for example:

- Orca journal: session, epoch, and sequence number
- JSONL transcript: file identity and byte offset or stable record identity
- SQLite provider history: provider-specific primary key or timestamp cursor

A changed or replaced source must be detected rather than silently continuing from an invalid cursor.

## Analysis

MVP analyzers should remain deterministic:

### Skill inventory

- installed count
- used count
- never used
- unused within N days
- calls per run
- last-used timestamp

### Tool usage

- call count
- failure count/rate
- repeated call patterns
- duration where available

### Run efficiency

- elapsed time
- interrupted/failed runs
- retry count
- review/fix loop count when lifecycle evidence supports it

## CLI boundaries

```text
orcalens scan
orcalens runs
orcalens skills
orcalens waste
orcalens report
```

Every command accepts `--json`. It writes one JSON object to stdout with a `command` field and a `data` object. Errors go to stderr and return a nonzero exit code.

The current `runs`, `skills`, and `waste` commands return an empty `items` array. `report` returns the stored run, skill inventory, and finding counts. The shared output contract gives future consumers the same result shape as the CLI.

## Dashboard

A dashboard is out of scope for this milestone. If added later, it should consume the same query/report layer as the CLI rather than querying internal tables directly. Candidate views include:

- overview
- task/run drill-down
- skills usage matrix
- agent x skill heatmap
- waste findings
- time/token trends

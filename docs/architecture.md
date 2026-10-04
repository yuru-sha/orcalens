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

Current Orca source inspection shows a host-level SQLite file named:

`agent-session-journal.db`

Important tables:

- `journal_sessions(session_id, workspace_id, epoch, ...)`
- `journal_rows(session_id, epoch, seq, ts, row_json)`

Rows are append-oriented and contain structured JSON. orcalens should read the current published epoch for each session and process rows in sequence order.

Do not couple analytics directly to a specific Orca journal schema version. The adapter should expose a stable internal event stream and report unsupported/newer schemas clearly.

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

`orcalens scan` should be idempotent.

Each source adapter should maintain a checkpoint based on the strongest stable cursor available, e.g.:

- Orca journal: session + epoch + seq
- JSONL transcript: file identity + byte offset or stable record identity
- SQLite provider history: provider-specific primary key / timestamp cursor

A changed/replaced source must be detected rather than silently continuing from an invalid cursor.

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

Machine-readable output should be supported from the beginning:

```text
--json
```

This will make a later dashboard consume the same stable reporting API rather than reaching directly into internal tables.

## Dashboard

Not part of the first milestone.

When added, it should consume the same query/report layer as the CLI and focus on:

- overview
- task/run drill-down
- skills usage matrix
- agent x skill heatmap
- waste findings
- time/token trends

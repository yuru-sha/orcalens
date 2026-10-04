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

The Rust crate exposes `scan`, `runs`, `skills`, `waste`, `report`, and `dashboard`. `reporting` owns the shared typed query contract (`ReportRequest -> ReportSnapshot`), including task/run/source-event evidence links, skill status and aggregates, waste analyzer results, and stored counts. CLI report commands serialize JSON with `command` and `data` fields; CLI code does not query database tables. The dashboard uses the same query layer through a read-only SQLite connection; `skills` retains its explicit filesystem inventory refresh.

`storage` opens the orcalens-owned database at `$HOME/.local/share/orcalens/orcalens.db`, or at the path in `ORCALENS_DB`. It enables SQLite foreign keys and applies embedded migrations in version order. `schema_migrations` records applied versions. Each migration and its ledger entry commit in one transaction.

On Unix, `storage` restricts the analytics database file to owner read and write. Raw Orca conversation data must not be readable by other local users.

`collectors` discovers and reads Orca journals without invoking Orca's reducer. `normalizers` recognizes only the explicit tool execution event shape described below. Provider transcripts are not collected. The dashboard reads normalized local data without scanning or refreshing it.

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
- run_id, nullable until run identity is supported by source evidence
- provider
- provider_session_id
- started_at
- ended_at

### ToolCall

- id
- run_id, nullable until run identity is supported by source evidence
- session_id
- call_id
- tool_name
- started_at
- ended_at
- status
- input_hash
- output_hash
- start_event_id
- end_event_id

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

The adapter opens the database read-only and imports row JSON unchanged into `raw_events`. It recognizes Orca `item` rows whose body is a `tool-call`, plus ordered tool-call mutations in `lifecycle-batch` rows. The journal `itemId` is the stable normalization key; the provider `callId` is nullable and preserved when present. `running`, `completed`, and `failed` map to `started`, `succeeded`, and `failed`. Input JSON is stored only as an FNV-1a 64-bit hash; a later non-null input revision replaces an earlier hash. The output hash uses Orca's bounded-output digest, avoiding a duplicate of the output body. An `interrupted` turn closes earlier started calls explicitly scoped to that turn from the same source, including prior epochs. Calls without turn scope remain `started`; a terminal item without a prior running revision has no `started_at`. An explicit terminal tool revision supersedes an inferred interruption. Normalization upgrades replay preserved prior-epoch raw events once, before processing the currently published rows; current rows are normalized on every scan.

Each Orca `turn` item creates or updates one normalized Run keyed by the source-qualified journal session and turn `itemId`. Its explicit `userItemId`, when nonempty and distinct from the turn ID, creates or updates a source-qualified Task; a later explicit attribution replaces the Run's previous Task link. Absent linkage leaves `task_id` null. A tool-call row's `turnScope.turnItemId` links the call to that Run. `run_sessions` records the explicit Run-to-provider-Session relationship and supports multiple sessions per Run. Run source-event references retain the journal rows that establish its lifecycle; tool calls retain their start/end row references and lifecycle-batch mutation order. The source's `workspace_id` is an opaque key, not a filesystem path or repository identity, so workspace and repository links stay unset rather than being guessed. The checkpoint stores the last sequence for each session and epoch, while normalization replays journal rows on later scans and upserts calls by `(source-qualified session_id, item_id)`. A changed epoch starts a new cursor while preserving earlier raw events. Unsupported schema versions and missing required fields fail with a diagnostic.

## Skill invocation detection

Skill parsing is isolated behind the provider-neutral `SkillInvocationDetector` interface. Initial adapters recognize surfaced `<command-name>/plugin:skill-name</command-name>` envelopes and the provider-specific Claude `Skill`, OpenCode `skill`, and OMP skill tools. Detection runs against imported Orca journal item rows and lifecycle mutations; external provider transcript collectors are not yet implemented.

`skill_inventory` is a refreshable snapshot of installed skills found in the supported user skill roots. `skill_calls` stores deduplicated calls with source-event provenance and nullable run/session links. Duplicate revisions of an item use a stable session/epoch/item/name key. The CLI exposes installed and observed skills independently, with call/run counts, last-used timestamp, and a configurable inactivity window. Never-used is not a deletion recommendation.

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

`analyzers::analyze` reads normalized tables and the stored `skill_inventory` snapshot; it does not scan the filesystem, mutate source data, or persist redundant findings. Findings are typed by `FindingKind`, include normalized evidence record IDs, affected run/session/skill/tool identifiers where available, a measured metric, threshold/baseline, and a plain-language explanation. Results are sorted by kind and affected entity for reproducible output.

The supported waste signals are:

- `never_invoked_skill`: an installed skill name with no matching `skill_calls` rows. All inventory entries for the skill appear in the evidence. Inventory may be stale, and low usage is not a deletion recommendation.
- `inactive_skill`: the latest stored call for an installed skill name is at least `--inactivity-days` before the newest numeric run, tool-call, or skill-call timestamp. The evidence includes the records that establish the calls and comparison clock.
- `repeated_skill_call`: at least `--repeat-count` calls to the same skill in one non-null run.
- `repeated_identical_tool_call`: at least `--repeat-count` calls with the same tool name and non-null normalized input hash in one run and session. This is exact hash equality, not proof of redundant work.
- `failed_tool_call`: a normalized tool call explicitly marked `failed`.
- `long_run`: a task-linked run whose numeric duration meets `--long-run-multiplier` times the median duration of at least three earlier completed runs for the same task. Only earlier runs count; the candidate is excluded. Without enough valid baseline observations, the analyzer emits no finding.
- `interrupted_run`: a run with explicit normalized `interrupted` outcome.

Defaults are 90 days, 2 calls, and a long-run multiplier of 2. The baseline minimum is three completed same-task runs. A completed run has numeric start and end timestamps, positive duration, and is not explicitly interrupted or unverifiable. It must end before the candidate starts. The median is calculated in milliseconds. Even-sized samples use the integer midpoint. The candidate must have a numeric start time. Findings with absent lifecycle timestamps are excluded from duration analysis. An inactive-skill finding requires numeric start timestamps for all observed calls of that skill and numeric latest stored evidence. If any matching call has a missing or nonnumeric start time, the analyzer omits the finding. Calls with missing hashes are excluded from identical-input comparison. The normalized schema cannot distinguish retries, near-identical inputs, abandonment, cycles between review and fixes, or token usage. It also lacks cross-task data for historical duration comparisons, so those signals are omitted rather than inferred.

## CLI boundaries

```text
orcalens scan
orcalens runs
orcalens skills
orcalens waste
orcalens report
orcalens dashboard
```

The reporting commands accept `--json`, write one JSON object to stdout, and return errors on stderr with a nonzero exit code. `dashboard` prints a local URL and serves the UI until Ctrl-C. It requires an initialized database and does not apply migrations.

`waste` returns `{command:"waste",data:{items:[...]}}` in JSON and accepts `--inactivity-days DAYS`, `--repeat-count COUNT`, and `--long-run-multiplier MULTIPLIER`. Analysis uses stored evidence. `runs` returns deterministic run records with explicit task links, nullable attribution/timestamps, normalized session/call links, and source-event evidence IDs. `skills` refreshes inventory and returns observed skill usage with Active, Dormant, NeverUsed, or Unknown status. `report` returns aggregate run, task, skill, call, stored finding, duration, and skill-status counts.

`ReportReader` exposes `ReportRequest -> ReportSnapshot` for paginated read-only queries, including normalized evidence. Evidence details return paged source-event metadata without raw payloads. A `SourceEvent` query returns a preview capped at 250,000 characters and includes the original byte count and hash; stored payloads remain unchanged.

## Dashboard

`orcalens dashboard` binds to an ephemeral port on `127.0.0.1` and serves a local dashboard. It opens the existing schema-v4 database in SQLite read-only and query-only modes. It does not create a database, run migrations, scan Orca sources, or refresh skill inventory. Requests with an unexpected Host or Origin are rejected; the server accepts only GET requests and limits request headers.

The overview shows run/task/skill/call counts, stored versus computed findings, skill-state counts, and run-duration buckets. The tasks, runs, skills, tools, agent/model, and waste views use the shared `ReportRequest -> ReportSnapshot` interface. Lists return 100 records by default and API requests are capped at 500. Task detail links to runs; run details and waste findings link to normalized records and original raw source events. Raw payload previews load only from event detail and stop at 250,000 characters; the response also reports the original byte count and hash, and the stored event remains unchanged.

Skill statuses use the current time and a 90-day default window. `active` means at least one valid observed call falls inside that window; `dormant` means all observed calls have valid timestamps outside it; `never_used` means an installed skill has no stored calls; `unknown` means timestamps cannot establish recency. Agent/model grouping uses only recorded run attribution. Token usage, provider transcript data, and retry or review/fix-loop claims are not shown without corresponding collected evidence.

Waste analysis computes findings from the full stored evidence set before the API pages the response. Pagination limits response size, not analyzer work.

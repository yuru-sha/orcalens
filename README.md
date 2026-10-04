# orcalens

Analyze Orca agent runs to find wasted time, tokens, tool calls, and unused skills.

orcalens is an external analytics/profiling tool for [stablyai/orca](https://github.com/stablyai/orca). It reads Orca session/journal data and agent transcripts, normalizes them into a local analytics database, and reports where agent workflows can be simplified or made more efficient.

## Goals

- Analyze Orca tasks, runs, sessions, agents, tools, and skills.
- Identify installed skills that are unused, rarely used, or dormant.
- Detect repeated work, retries, review/fix loops, and stalled runs.
- Compare agent/model behavior and cost across similar work.
- Keep Orca integration read-only wherever possible.
- Preserve raw source events so parsers can be improved without rescanning original data.

## Non-goals

- Replacing Orca.
- Controlling or scheduling agents.
- Modifying Orca's own journal database.
- Declaring a skill useless solely because it has low call volume.

## Initial data sources

### Orca journal

Current Orca builds persist structured chat history in:

`agent-session-journal.db`

The database contains at least:

- `journal_sessions`
- `journal_rows`

The journal rows provide session identity, workspace identity, timestamps, lifecycle events, producer/agent linkage, tool activity, and structured turn information.

### Agent transcripts

Some information, especially skill invocation, may be easier or only possible to recover from provider transcripts. Initial adapters should target the providers commonly used through Orca:

- Codex
- Oh-My-Pi
- Claude Code
- OpenCode

Provider-specific parsing must stay behind adapters.

## Architecture

```text
Orca journal / provider transcripts / workspace metadata
                        |
                        v
                   collectors
                        |
                        v
                    raw_events
                        |
                        v
                   normalizers
                        |
          +-------------+-------------+
          |             |             |
        runs        tool_calls     skill_calls
          |             |             |
          +-------------+-------------+
                        |
                        v
                    analyzers
                        |
          +-------------+-------------+
          |             |             |
        report        skills         waste
```

See [docs/architecture.md](docs/architecture.md).

## CLI

Build and run the CLI with Cargo:

```sh
cargo run -- --help
cargo run -- scan
cargo run -- runs --json
```

The CLI provides `scan`, `runs`, `skills`, `waste`, and `report`. Each command accepts `--json` before or after the command and writes one JSON object to stdout. Errors go to stderr with a nonzero exit code.

`orcalens skills` refreshes local installed-skill inventory and reports observed invocations, call/run counts, and last-use timestamps. `orcalens skills --unused` filters never-used or dormant skills with a 90-day default configurable by `--inactivity-days DAYS`. Never-used does not mean safe to delete. Provider transcripts outside Orca journals are not collected.

The `reporting` module owns the shared typed query layer used by the CLI, including run/task summaries, skill usage/status, waste findings, and aggregate counts; CLI code does not query database tables. A dashboard follow-up must consume this report layer instead of issuing direct database queries.

## Local database

Each command opens the orcalens-owned SQLite database and applies pending migrations. The default path is `$HOME/.local/share/orcalens/orcalens.db`. Set `ORCALENS_DB` to use another file.

The migration ledger records each applied version. Reopening the database does not apply an already recorded migration again. On Unix, the analytics database file has owner-only read and write permissions because it stores raw conversation data. `scan` reads `agent-session-journal.db` from directories in `ORCA_STATE_DIRS` or `ORCA_STATE_DIR`, then checks `$HOME/.orca` and `$HOME/.local/share/orca`. It opens each journal read-only. The checkpoint tracks each session's published epoch and sequence.

Unsupported journal schema versions or missing required tables and columns stop the scan with a diagnostic. The collector keeps each row's JSON payload unchanged in `raw_events`. It normalizes Orca tool-call items and ordered lifecycle-batch mutations: `running`, `completed`, and `failed` become `started`, `succeeded`, and `failed`. Input values are stored as hashes; later non-null revisions replace earlier input hashes, and output hashes reuse Orca's bounded-output digest. An interrupted turn closes earlier calls explicitly scoped to that turn in the same source, including prior epochs. Existing raw events from prior epochs are replayed once during normalization upgrades; current published rows continue to be normalized on every scan.

## Scope

Each Orca turn `itemId` becomes a Run; its explicit `userItemId`, when present and distinct from the turn ID, links a Task, and tool-call `turnScope` links calls to that Run. Later explicit request attribution updates the Run's Task link. Task/run/session identities are source-qualified to prevent unrelated journals with reused provider IDs from merging. The join table supports multiple Sessions per Run. Workspace IDs are opaque, not paths, so scan does not assign workspace or repository links. Missing explicit Task linkage remains null; no heuristic relationship confidence is fabricated. Provider transcripts are not collected. The dashboard is a planned follow-up and must use the shared reporting API.

## Waste signals

Implemented deterministic signals:

- installed skills with no matching stored invocation (the refreshed inventory snapshot can be stale)
- skills whose last stored invocation is at least N days before the newest timestamp in stored run/tool/skill evidence
- repeated skill calls in a run
- repeated tool calls in a run with the same tool name and stored input hash
- explicitly failed tool calls
- runs at least a configurable multiple of the median duration among three or more earlier completed runs for the same task
- runs with an explicit `interrupted` outcome

Configure `orcalens waste` with `--inactivity-days DAYS`, `--repeat-count COUNT`, and `--long-run-multiplier MULTIPLIER` (defaults: 90, 2, and 2). A long-run finding needs a numeric start time and at least three completed prior runs for the same task. Each baseline run must end before the candidate starts. The analyzer calculates the median from those runs.

Inactivity compares call timestamps with the newest numeric timestamp in stored runs, tool calls, or skill calls. It does not use wall-clock time. If any matching skill call has a missing or nonnumeric start timestamp, the analyzer omits that finding. Findings cite the records that establish the signal. A repeated hash does not prove redundant work. An interrupted outcome does not establish why work stopped.

The current schema does not distinguish tool retries, near-identical inputs, abandonment, cycles between review and fixes, or token usage. These signals are not reported. Runs without sufficient same-task history do not receive long-run findings. Waste analysis reads stored inventory and normalized evidence without refreshing the filesystem inventory. Refresh inventory with `orcalens skills` when needed. Low usage is never a deletion recommendation.

## Technology

Initial implementation language: **Rust**

Storage: **SQLite**

The application is CLI-first. The task/run dashboard is a follow-up to the reporting/query API.

## Status

The current implementation contains the CLI, the shared typed reporting/query API, migration-managed SQLite schema, a read-only Orca journal collector, normalized skill invocation detection, and deterministic evidence-based waste analyzers.

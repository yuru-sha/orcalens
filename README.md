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

The CLI provides `scan`, `runs`, `skills`, `waste`, and `report`. Each command accepts `--json` before or after the command. `orcalens skills` lists installed skills and observed invocations, including call/run counts and last-use timestamps; `orcalens skills --unused` filters never-used or inactive skills, with a 90-day default configurable by `--inactivity-days DAYS`. Never-used does not mean safe to delete.

JSON writes one object to stdout with a `command` field and a `data` object. Errors go to stderr and return a nonzero exit code. `scan` imports Orca journal rows and normalizes structured tool calls. The skills query refreshes local installed-skill inventory and detects surfaced invocation envelopes and provider Skill tool calls in imported Orca journal events. Provider transcripts outside Orca journals are not collected.

## Local database

Each command opens the orcalens-owned SQLite database and applies pending migrations before it runs. The default path is `$HOME/.local/share/orcalens/orcalens.db`. Set `ORCALENS_DB` to use another file.

The migration ledger records each applied version. Reopening the database does not apply an already recorded migration again. On Unix, the analytics database file has owner-only read and write permissions because it stores raw conversation data. `scan` reads `agent-session-journal.db` from directories in `ORCA_STATE_DIRS` or `ORCA_STATE_DIR`, then checks `$HOME/.orca` and `$HOME/.local/share/orca`. It opens each journal read-only. The checkpoint tracks each session's published epoch and sequence.

Unsupported journal schema versions or missing required tables and columns stop the scan with a diagnostic. The collector keeps each row's JSON payload unchanged in `raw_events`. It normalizes Orca tool-call items and ordered lifecycle-batch mutations: `running`, `completed`, and `failed` become `started`, `succeeded`, and `failed`. Input values are stored as hashes; later non-null revisions replace earlier input hashes, and output hashes reuse Orca's bounded-output digest. An interrupted turn closes earlier calls explicitly scoped to that turn in the same source, including prior epochs. Existing raw events from prior epochs are replayed once during normalization upgrades; current published rows continue to be normalized on every scan.

## Scope

Each Orca turn `itemId` becomes a Run; its explicit `userItemId`, when present and distinct from the turn ID, links a Task, and tool-call `turnScope` links calls to that Run. Later explicit request attribution updates the Run's Task link. Task/run/session identities are source-qualified to prevent unrelated journals with reused provider IDs from merging. The join table supports multiple Sessions per Run. Workspace IDs are opaque, not paths, so scan does not assign workspace or repository links. Missing explicit Task linkage remains null; no heuristic relationship confidence is fabricated. Provider transcripts and the dashboard are out of scope.

## Waste signals

Planned deterministic signals:

- never-used installed skills
- skills unused for a configurable period
- repeated skill invocation in one run
- repeated identical or near-identical tool calls
- failed/retried tool calls
- unusually long runs
- repeated review -> fix cycles
- interrupted or abandoned runs

Recommendations should be evidence-based. Low-frequency specialist skills should not be automatically marked for deletion.

## Technology

Initial implementation language: **Rust**

Storage: **SQLite**

The application is CLI-first. A dashboard is out of scope.

## Status

The current implementation contains the CLI, migration-managed SQLite schema, and a read-only Orca journal collector. Normalizers and analyzers remain empty.

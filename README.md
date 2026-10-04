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

The CLI provides `scan`, `runs`, `skills`, `waste`, and `report`. Each command accepts `--json` before or after the command.

JSON writes one object to stdout with a `command` field and a `data` object. Errors go to stderr and return a nonzero exit code. `scan` reports zero scanned sources and imported events because no collectors exist yet. `runs`, `skills`, and `waste` return an empty `items` array. `report` returns counts from the orcalens database.

## Local database

Each command opens the orcalens-owned SQLite database and applies pending migrations before it runs. The default path is `$HOME/.local/share/orcalens/orcalens.db`. Set `ORCALENS_DB` to use another file.

The migration ledger records each applied version. Reopening the database does not apply an already recorded migration again. The CLI does not open Orca databases or provider transcripts. Future collectors must treat both as read-only.

## Scope

The current change establishes the CLI and local storage only. It does not collect or analyze Orca data. The dashboard is out of scope.

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

The current implementation contains the CLI, migration-managed SQLite schema, and empty collector, normalizer, and analyzer modules.

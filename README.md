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

## MVP

The first useful milestone is deliberately narrow:

> Compare the installed skill inventory with skills that were actually invoked.

The MVP should also provide enough run/session/tool data to explain where those skill calls came from.

Planned CLI:

```text
orcalens scan
orcalens runs
orcalens skills
orcalens waste
orcalens report
```

Example:

```text
$ orcalens skills --unused

Skill                         Calls   Runs   Last used   Status
----------------------------------------------------------------
go/go-testing                  181     92    today       active
openapi/review                  18     11    14d ago     low
database/sqlite                  3      2    41d ago     dormant
legacy-api-client                0      0    never       unused
```

## Waste signals

Initial deterministic signals:

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

The first version should be CLI-first. A local web dashboard can be added after collection and normalization are reliable.

## Status

Early design / bootstrap.

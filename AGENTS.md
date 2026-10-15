# Repository Guidelines

## Project Overview

`orcalens` is a Rust CLI and local read-only dashboard for analyzing Orca agent runs. It imports Orca journal evidence into an orcalens-owned SQLite database, normalizes events, and produces deterministic reports and evidence-backed waste findings. Keep source integration read-only; provider transcripts are not currently collected.

## Architecture & Data Flow

Core flow: Orca journal discovery and collection (`collectors`) → preserved raw events → typed normalization (`normalizers`) → SQLite entities → shared reporting queries (`reporting`) and deterministic waste analysis (`analyzers`) → CLI or dashboard output. `skills` refreshes installed-skill inventory and reports stored invocations. Provider-specific parsing belongs behind adapters; analysis should consume normalized, provider-neutral records. Keep Task, Run, and Session identities distinct and relationships explicit—do not infer missing links or unsupported metrics.

The dashboard uses the shared `ReportReader` / `ReportRequest -> ReportSnapshot` query contract and a read-only, query-only database connection; avoid direct dashboard SQL against internal tables. It serves embedded assets on loopback. Raw source evidence should remain available to explain results; findings must be reproducible and grounded in stored evidence.

## Key Directories

- `src/`: CLI entry point, storage, journal collection/normalization, reporting, analyzers, skills, and dashboard assets.
- `tests/`: integration/contract tests for CLI behavior, reporting, and analyzers.
- `migrations/`: ordered embedded SQLite schema migrations; add schema changes as a new migration rather than editing applied history.
- `docs/`: architecture and data/behavior contracts.

## Development Commands

```sh
cargo run -- --help
cargo run -- scan
cargo run -- runs --json
cargo run -- dashboard
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets

```

CLI commands: `scan`, `runs`, `skills`, `waste`, `report`, `dashboard`. Reporting commands support `--json`; JSON is emitted to stdout and errors to stderr with a nonzero exit code. `dashboard` serves until Ctrl-C and requires an existing schema-v4 database; it does not migrate or scan sources. Other commands open the writable analytics DB and apply pending migrations.

## Code Conventions & Common Patterns

- Propagate fallible operations with `Result` and `?` rather than swallowing errors; for example, `src/cli.rs` returns `Result<(), Box<dyn Error>>` and propagates storage and reporting failures with `?`.
- Use typed request/result structures and explicit evidence references rather than passing unstructured query state through the CLI or dashboard.
- Make scans idempotent and transactional; preserve original raw event payloads and normalize only recognized event shapes.
- Waste findings are deterministic and evidence-backed. Do not treat repeated hashes as proof of redundant work, low skill use as a deletion recommendation, or absent attribution as grounds for a heuristic link.
- Keep local Orca/database data private. The analytics DB is owner-only on Unix; source journals are opened read-only.

## Important Files

- `src/main.rs`, `src/cli.rs`: binary entry point and command interface.
- `src/collectors.rs`, `src/normalizers.rs`: source discovery/import and event normalization.
- `src/storage.rs`, `migrations/`: database setup and schema.
- `src/reporting.rs`, `src/analyzers.rs`: shared read API and waste rules.
- `src/dashboard.rs` and `src/dashboard.{html,css,js}`: local dashboard server and UI.
- `tests/*_contract.rs`, `tests/waste_cli.rs`: integration behavior contracts.
- `README.md`, `docs/architecture.md`: user commands, supported behavior, and design constraints.

## Runtime/Tooling Preferences

Use Cargo and Rust (edition 2021); the project is a single Rust package. `rusqlite` uses bundled SQLite. `rust-analyzer` is configured in `.omp/lsp.json`. Runtime DB path defaults to `$HOME/.local/share/orcalens/orcalens.db`; override with `ORCALENS_DB`. Source discovery honors `ORCA_STATE_DIRS` or `ORCA_STATE_DIR`, then checks `$HOME/.orca` and `$HOME/.local/share/orca`. No repository-specific scripts or alternate package manager are required.

## Testing & QA

Run `cargo test` after behavior changes. Integration tests use Rust's built-in test framework and temporary SQLite fixtures (`tempfile`); CLI contracts invoke the compiled binary and isolate home/database paths. Add tests for consumer-visible behavior, data relationships, evidence, thresholds, error paths, and JSON/human output—not implementation details. No coverage target or coverage tool is configured. For changes to ingestion or schema, preserve read-only source access, transactional migration behavior, and repeat-scan correctness; for dashboard changes, verify it reads existing data without creating/migrating the DB or refreshing inventory.

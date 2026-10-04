use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "normalized_entities",
        sql: include_str!("../migrations/0002_normalized_entities.sql"),
    },
    Migration {
        version: 3,
        name: "turn_runs",
        sql: include_str!("../migrations/0003_turn_runs.sql"),
    },
    Migration {
        version: 4,
        name: "skill_usage",
        sql: include_str!("../migrations/0004_skill_usage.sql"),
    },
];

pub fn default_database_path() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = std::env::var_os("ORCALENS_DB") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("orcalens")
        .join("orcalens.db"))
}

pub fn open_default() -> Result<Connection, Box<dyn Error>> {
    open(default_database_path()?)
}

pub fn open(path: impl AsRef<Path>) -> Result<Connection, Box<dyn Error>> {
    let path = path.as_ref();
    if path != Path::new(":memory:") {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
    }
    let connection = Connection::open(path)?;
    #[cfg(unix)]
    if path != Path::new(":memory:") {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    migrate(connection)
}

fn migrate(mut connection: Connection) -> Result<Connection, Box<dyn Error>> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        ) STRICT;",
    )?;

    let applied: Vec<(i64, String)> = {
        let mut statement =
            transaction.prepare("SELECT version, name FROM schema_migrations ORDER BY version")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        rows
    };
    if applied.len() > MIGRATIONS.len() {
        return Err("database contains unsupported schema migrations".into());
    }
    for (index, (version, name)) in applied.iter().enumerate() {
        let expected = &MIGRATIONS[index];
        if *version != expected.version || name != expected.name {
            return Err(format!(
                "database migration ledger is not a contiguous prefix at version {}",
                expected.version
            )
            .into());
        }
    }
    for migration in MIGRATIONS.iter().skip(applied.len()) {
        if migration.version == 2 {
            validate_legacy_links(&transaction)?;
        }
        apply_migration(&transaction, migration)?;
        transaction.execute(
            "INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)",
            (migration.version, migration.name),
        )?;
    }
    transaction.commit()?;

    Ok(connection)
}

fn apply_migration(
    transaction: &Transaction<'_>,
    migration: &Migration,
) -> Result<(), rusqlite::Error> {
    transaction.execute_batch(migration.sql)
}

fn validate_legacy_links(transaction: &Transaction<'_>) -> Result<(), Box<dyn Error>> {
    for (table, query) in [
        (
            "sessions.run_id",
            "SELECT EXISTS(SELECT 1 FROM sessions s LEFT JOIN runs r ON r.id = s.run_id WHERE s.run_id IS NOT NULL AND r.id IS NULL)",
        ),
        (
            "tool_calls.run_id",
            "SELECT EXISTS(SELECT 1 FROM tool_calls c LEFT JOIN runs r ON r.id = c.run_id WHERE c.run_id IS NOT NULL AND r.id IS NULL)",
        ),
        (
            "tool_calls.session_id",
            "SELECT EXISTS(SELECT 1 FROM tool_calls c LEFT JOIN sessions s ON s.id = c.session_id WHERE c.session_id IS NOT NULL AND s.id IS NULL)",
        ),
        (
            "skill_calls.run_id",
            "SELECT EXISTS(SELECT 1 FROM skill_calls c LEFT JOIN runs r ON r.id = c.run_id WHERE c.run_id IS NOT NULL AND r.id IS NULL)",
        ),
        (
            "skill_calls.session_id",
            "SELECT EXISTS(SELECT 1 FROM skill_calls c LEFT JOIN sessions s ON s.id = c.session_id WHERE c.session_id IS NOT NULL AND s.id IS NULL)",
        ),
    ] {
        let has_orphan: bool = transaction.query_row(query, [], |row| row.get(0))?;
        if has_orphan {
            return Err(format!(
                "cannot migrate normalized entities: legacy {table} contains an orphaned reference"
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::open;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn migration_is_idempotent_and_survives_reopening() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("orcalens.db");

        {
            let connection = open(&path).expect("first open");
            let migrations: i64 = connection
                .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .expect("migration count");
            assert_eq!(migrations, 4);
        }
        {
            let connection = open(&path).expect("reopen");
            let migrations: i64 = connection
                .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .expect("migration count after reopen");
            let tables: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'runs'",
                    [],
                    |row| row.get(0),
                )
                .expect("runs table");
            assert_eq!(migrations, 4);
            assert_eq!(tables, 1);
        }
    }
    #[cfg(unix)]
    #[test]
    fn database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("orcalens.db");
        open(&path).expect("open database");
        let mode = std::fs::metadata(path)
            .expect("database metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    #[test]
    fn in_memory_database_opens() {
        let connection = open(":memory:").expect("open in-memory database");
        let migrations: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration count");
        assert_eq!(migrations, 4);
    }
    #[test]
    fn upgrades_existing_initial_schema_and_preserves_normalized_rows() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("orcalens.db");
        {
            let connection = rusqlite::Connection::open(&path).expect("create legacy database");
            connection
                .execute_batch(include_str!("../migrations/0001_initial.sql"))
                .expect("apply initial schema");
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (
                         version INTEGER PRIMARY KEY,
                         name TEXT NOT NULL,
                         applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ) STRICT;
                     INSERT INTO schema_migrations (version, name) VALUES (1, 'initial');
                     INSERT INTO tasks (source, source_key) VALUES ('fixture', 'task-a');
                     INSERT INTO runs (task_id) VALUES (1);
                     INSERT INTO sessions (run_id, provider, provider_session_id) VALUES (1, 'fixture', 'session-a');
                     INSERT INTO tool_calls (run_id, session_id, call_id, tool_name, status)
                     VALUES (1, 1, 'call-a', 'read', 'succeeded');
                     INSERT INTO skill_calls (run_id, session_id, skill_name, source)
                     VALUES (1, 1, 'review', 'fixture');",
                )
                .expect("add existing normalized rows");
        }
        let connection = open(&path).expect("upgrade legacy database");
        let preserved: (i64, i64, String, Option<i64>, String) = connection
            .query_row(
                "SELECT tc.id, s.id, tc.call_id, tc.run_id, tc.status
                 FROM tool_calls tc JOIN sessions s ON s.id = tc.session_id",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("preserved rows");
        assert_eq!(
            preserved,
            (1, 1, "call-a".to_owned(), Some(1), "succeeded".to_owned())
        );
        let violations: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("foreign key check");
        assert_eq!(violations, 0);
        let skill_session: Option<i64> = connection
            .query_row("SELECT session_id FROM skill_calls", [], |row| row.get(0))
            .expect("preserved skill session");
        assert_eq!(skill_session, Some(1));
        let migrated_relationships: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM run_sessions WHERE run_id = 1 AND session_id = 1",
                [],
                |row| row.get(0),
            )
            .expect("migrated legacy session-run relationship");
        assert_eq!(migrated_relationships, 1);
        connection
            .execute_batch(
                "INSERT INTO runs (task_id) VALUES (1);
                 INSERT INTO sessions (run_id, provider, provider_session_id) VALUES (1, 'fixture', 'session-b');
                 INSERT INTO sessions (run_id, provider, provider_session_id) VALUES (2, 'fixture', 'session-c');
                 INSERT INTO run_sessions (run_id, session_id) VALUES (1, 2), (2, 3);",
            )
            .expect("allow multiple runs and sessions");
        let linked_counts: (i64, i64) = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM runs WHERE task_id = 1),
                    (SELECT COUNT(*) FROM run_sessions WHERE run_id = 1)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("entity cardinalities");
        assert_eq!(linked_counts, (2, 2));
    }
    #[test]
    fn refuses_to_rebuild_legacy_rows_with_orphaned_links() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("orcalens.db");
        {
            let connection = rusqlite::Connection::open(&path).expect("create legacy database");
            connection
                .execute_batch(include_str!("../migrations/0001_initial.sql"))
                .expect("apply initial schema");
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (
                         version INTEGER PRIMARY KEY,
                         name TEXT NOT NULL,
                         applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ) STRICT;
                     INSERT INTO schema_migrations (version, name) VALUES (1, 'initial');
                     INSERT INTO tasks (source, source_key) VALUES ('fixture', 'task-a');
                     INSERT INTO runs (task_id) VALUES (1);
                     INSERT INTO sessions (run_id, provider, provider_session_id) VALUES (1, 'fixture', 'session-a');
                     PRAGMA foreign_keys = OFF;
                     INSERT INTO tool_calls (run_id, session_id, call_id, tool_name, status)
                     VALUES (99, 99, 'orphan', 'read', 'succeeded');",
                )
                .expect("add orphaned legacy row");
        }

        let error = open(&path).expect_err("refuse orphaned references");
        assert!(
            error
                .to_string()
                .contains("legacy tool_calls.run_id contains an orphaned reference"),
            "unexpected migration error: {error}"
        );
        let connection = rusqlite::Connection::open(&path).expect("reopen preserved database");
        let migration_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration ledger");
        let orphan_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE call_id = 'orphan'",
                [],
                |row| row.get(0),
            )
            .expect("orphan row");
        assert_eq!(migration_count, 1);
        assert_eq!(orphan_count, 1);
    }

    #[test]
    fn concurrent_first_opens_apply_migrations_once() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("orcalens.db");
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let path = path.clone();
                thread::spawn(move || {
                    barrier.wait();
                    let connection = open(path).expect("concurrent open");
                    connection
                        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .expect("migration count")
                })
            })
            .collect();

        for handle in handles {
            assert_eq!(handle.join().expect("open thread"), 4);
        }
    }
}

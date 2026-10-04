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

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sql: include_str!("../migrations/0001_initial.sql"),
}];

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
            assert_eq!(migrations, 1);
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
            assert_eq!(migrations, 1);
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
        assert_eq!(migrations, 1);
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
            assert_eq!(handle.join().expect("open thread"), 1);
        }
    }
}

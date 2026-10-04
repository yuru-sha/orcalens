use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

const JOURNAL_NAME: &str = "agent-session-journal.db";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub sources_scanned: u64,
    pub events_imported: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Checkpoint {
    sessions: BTreeMap<String, SessionCursor>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SessionCursor {
    epoch: String,
    seq: i64,
}

#[derive(Debug)]
struct JournalRow {
    session_id: String,
    epoch: String,
    seq: i64,
    timestamp: i64,
    payload: String,
}

pub fn discover_journals() -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut directories = Vec::new();
    if let Some(configured) = std::env::var_os("ORCA_STATE_DIRS") {
        directories.extend(std::env::split_paths(&configured));
    }
    if let Some(configured) = std::env::var_os("ORCA_STATE_DIR") {
        directories.push(PathBuf::from(configured));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        directories.push(home.join(".orca"));
        directories.push(home.join(".local").join("share").join("orca"));
    }
    let mut journals = Vec::new();
    for directory in directories {
        let path = directory.join(JOURNAL_NAME);
        if path.is_file() {
            journals.push(path.canonicalize()?);
        }
    }
    journals.sort();
    journals.dedup();
    Ok(journals)
}

pub fn scan(connection: &mut Connection) -> Result<ScanSummary, Box<dyn Error>> {
    let journals = discover_journals()?;
    let mut summary = ScanSummary::default();
    for path in journals {
        summary.events_imported += scan_journal(connection, &path)?;
        summary.sources_scanned += 1;
    }
    Ok(summary)
}

fn scan_journal(destination: &mut Connection, path: &Path) -> Result<u64, Box<dyn Error>> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(std::time::Duration::from_secs(5))?;
    validate_journal(&source)?;
    let source_identity = path.to_string_lossy().into_owned();
    let transaction = destination.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO sources (source_type, source_path, source_identity)
         VALUES ('orca_journal', ?1, ?1)
         ON CONFLICT (source_type, source_path, source_identity) DO NOTHING",
        [&source_identity],
    )?;
    let source_id: i64 = transaction.query_row(
        "SELECT id FROM sources WHERE source_type = 'orca_journal' AND source_path = ?1 AND source_identity = ?1",
        [&source_identity],
        |row| row.get(0),
    )?;
    let cursor_json: Option<String> = transaction
        .query_row(
            "SELECT cursor FROM scan_checkpoints WHERE source_id = ?1",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    let mut checkpoint: Checkpoint = cursor_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default();
    let mut statement = source.prepare(
        "SELECT r.session_id, r.epoch, r.seq, r.ts, r.row_json
         FROM journal_rows r
         JOIN journal_sessions s ON s.session_id = r.session_id
         WHERE r.epoch = s.epoch
         ORDER BY r.session_id, r.epoch, r.seq",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(JournalRow {
            session_id: row.get(0)?,
            epoch: row.get(1)?,
            seq: row.get(2)?,
            timestamp: row.get(3)?,
            payload: row.get(4)?,
        })
    })?;
    let mut imported = 0;
    for row in rows {
        let row = row?;
        let prior = checkpoint.sessions.get(&row.session_id);
        if prior.is_some_and(|cursor| cursor.epoch == row.epoch && cursor.seq >= row.seq) {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(&row.payload).map_err(|error| {
            format!(
                "invalid Orca journal JSON for session {} epoch {} seq {}: {error}",
                row.session_id, row.epoch, row.seq
            )
        })?;
        let identity = format!("{}:{}:{}", row.session_id, row.epoch, row.seq);
        transaction.execute(
            "INSERT INTO raw_events (source_id, source_identity, observed_at, source_timestamp, payload, payload_hash)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?3, ?4, ?5)
             ON CONFLICT (source_id, source_identity) DO NOTHING",
            rusqlite::params![source_id, identity, row.timestamp.to_string(), row.payload, payload_hash(&row.payload)],
        )?;
        imported += transaction.changes();
        checkpoint.sessions.insert(
            row.session_id,
            SessionCursor {
                epoch: row.epoch,
                seq: row.seq,
            },
        );
    }
    transaction.execute(
        "INSERT INTO scan_checkpoints (source_id, cursor) VALUES (?1, ?2)
         ON CONFLICT (source_id) DO UPDATE SET cursor = excluded.cursor, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![source_id, serde_json::to_string(&checkpoint)?],
    )?;
    transaction.commit()?;
    Ok(imported)
}

fn validate_journal(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(3..=4).contains(&version) {
        return Err(format!(
            "unsupported Orca journal schema version {version}; supported versions are 3 and 4"
        )
        .into());
    }
    for (table, required) in [
        ("journal_sessions", &["session_id", "epoch"][..]),
        (
            "journal_rows",
            &["session_id", "epoch", "seq", "ts", "row_json"][..],
        ),
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(format!("unsupported Orca journal schema: missing table {table}").into());
        }
        let mut statement = connection.prepare(&format!("PRAGMA table_info('{table}')"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(column) = required
            .iter()
            .find(|column| !columns.iter().any(|found| found == **column))
        {
            return Err(format!(
                "unsupported Orca journal schema: {table} is missing column {column}"
            )
            .into());
        }
    }
    Ok(())
}

fn payload_hash(payload: &str) -> String {
    let hash = payload.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture(path: &Path) {
        let connection = Connection::open(path).expect("create journal");
        connection.execute_batch(
            "CREATE TABLE journal_sessions (session_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, epoch TEXT NOT NULL);
             CREATE TABLE journal_rows (session_id TEXT NOT NULL, epoch TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL, row_json TEXT NOT NULL, PRIMARY KEY(session_id, epoch, seq));
             PRAGMA user_version = 4;
             INSERT INTO journal_sessions VALUES ('session-a', '/tmp', 'epoch-1');
             INSERT INTO journal_rows VALUES ('session-a', 'epoch-1', 1, 1767225600000, '{\"type\":\"message\"}');",
        ).expect("create journal fixture");
    }

    #[test]
    fn scan_is_incremental_and_imports_replacement_epochs() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        let database = directory.path().join("orcalens.db");
        fixture(&journal);
        let mut destination = crate::storage::open(&database).expect("open destination");
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("first scan"),
            1
        );
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("repeat scan"),
            0
        );
        Connection::open(&journal)
            .expect("open journal")
            .execute(
                "INSERT INTO journal_rows VALUES ('session-a', 'epoch-1', 2, 1767225601000, '{\"type\":\"next\"}')",
                [],
            )
            .expect("append journal row");
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("append scan"),
            1
        );
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("repeat append scan"),
            0
        );
        Connection::open(&journal).expect("open journal").execute_batch(
            "UPDATE journal_sessions SET epoch = 'epoch-2';
             INSERT INTO journal_rows VALUES ('session-a', 'epoch-2', 1, 1767312000000, '{\"type\":\"replacement\"}');"
        ).expect("replace epoch");
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("replacement scan"),
            1
        );
        let count: i64 = destination
            .query_row("SELECT COUNT(*) FROM raw_events", [], |row| row.get(0))
            .expect("event count");
        assert_eq!(count, 3);
    }

    #[test]
    fn scans_live_wal_journal_without_changing_source_files() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let writer = Connection::open(&journal).expect("open journal writer");
        writer
            .pragma_update(None, "journal_mode", "WAL")
            .expect("enable WAL");
        writer
            .execute(
                "INSERT INTO journal_rows VALUES ('session-a', 'epoch-1', 2, 1767225601000, '{\"type\":\"live\"}')",
                [],
            )
            .expect("write WAL row");
        let row_count: i64 = writer
            .query_row("SELECT COUNT(*) FROM journal_rows", [], |row| row.get(0))
            .expect("source row count");
        let version: i64 = writer
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("source schema version");
        let database_before = std::fs::read(&journal).expect("read journal file");
        let wal_path = PathBuf::from(format!("{}-wal", journal.display()));
        let wal_before = std::fs::read(&wal_path).expect("read journal WAL");
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("scan live WAL"),
            2
        );
        assert_eq!(
            std::fs::read(&journal).expect("read journal file after scan"),
            database_before
        );
        assert_eq!(
            std::fs::read(&wal_path).expect("read journal WAL after scan"),
            wal_before
        );
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM journal_rows", [], |row| row
                    .get::<_, i64>(0))
                .expect("source row count after scan"),
            row_count
        );
        assert_eq!(
            writer
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("source schema version after scan"),
            version
        );
    }

    #[test]
    fn rejects_newer_journal_schema_before_destination_write() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        Connection::open(&journal)
            .expect("open journal")
            .pragma_update(None, "user_version", 9)
            .expect("set version");
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");
        let error = scan_journal(&mut destination, &journal).expect_err("reject schema");
        assert!(error
            .to_string()
            .contains("unsupported Orca journal schema version 9"));
        let count: i64 = destination
            .query_row("SELECT COUNT(*) FROM sources", [], |row| row.get(0))
            .expect("source count");
        assert_eq!(count, 0);
    }
}

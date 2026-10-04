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
#[serde(default)]
struct Checkpoint {
    sessions: BTreeMap<String, SessionCursor>,
    normalization_backfill_complete: bool,
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
    if !checkpoint.normalization_backfill_complete {
        normalize_persisted_source_events(&transaction, source_id)?;
        checkpoint.normalization_backfill_complete = true;
    }
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
        let already_checkpointed =
            prior.is_some_and(|cursor| cursor.epoch == row.epoch && cursor.seq >= row.seq);
        serde_json::from_str::<serde_json::Value>(&row.payload).map_err(|error| {
            format!(
                "invalid Orca journal JSON for session {} epoch {} seq {}: {error}",
                row.session_id, row.epoch, row.seq
            )
        })?;
        let identity = format!("{}:{}:{}", row.session_id, row.epoch, row.seq);
        if !already_checkpointed {
            transaction.execute(
                "INSERT INTO raw_events (source_id, source_identity, observed_at, source_timestamp, payload, payload_hash)
                 VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?3, ?4, ?5)
                 ON CONFLICT (source_id, source_identity) DO NOTHING",
                rusqlite::params![source_id, identity, row.timestamp.to_string(), row.payload, payload_hash(&row.payload)],
            )?;
            imported += transaction.changes();
        }
        let raw_event_id: i64 = transaction.query_row(
            "SELECT id FROM raw_events WHERE source_id = ?1 AND source_identity = ?2",
            rusqlite::params![source_id, identity],
            |row| row.get(0),
        )?;
        crate::normalizers::normalize_journal_row(
            &transaction,
            crate::normalizers::JournalSourceRow {
                source_id,
                provider_session_id: &row.session_id,
                epoch: &row.epoch,
                sequence: row.seq,
                timestamp: row.timestamp,
                raw_event_id,
                mutation_index: 0,
            },
            &row.payload,
        )?;
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

fn normalize_persisted_source_events(
    transaction: &rusqlite::Transaction<'_>,
    source_id: i64,
) -> Result<(), Box<dyn Error>> {
    let events = {
        let mut statement = transaction.prepare(
            "SELECT id, source_identity, source_timestamp, payload
             FROM raw_events WHERE source_id = ?1 ORDER BY id",
        )?;
        let rows = statement
            .query_map([source_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (raw_event_id, identity, source_timestamp, payload) in events {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        let (Some(epoch), Some(sequence)) = (
            row.get("epoch").and_then(serde_json::Value::as_str),
            row.get("seq").and_then(serde_json::Value::as_i64),
        ) else {
            continue;
        };
        let Some(session_id) = identity.strip_suffix(&format!(":{epoch}:{sequence}")) else {
            continue;
        };
        let Some(timestamp) = source_timestamp
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok())
        else {
            continue;
        };
        crate::normalizers::normalize_journal_row(
            transaction,
            crate::normalizers::JournalSourceRow {
                source_id,
                provider_session_id: session_id,
                epoch,
                sequence,
                timestamp,
                raw_event_id,
                mutation_index: 0,
            },
            &payload,
        )?;
    }
    Ok(())
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

    struct NormalizedCall {
        call_id: String,
        status: String,
        started_at: Option<String>,
        ended_at: Option<String>,
        input_hash: Option<String>,
        output_hash: Option<String>,
    }

    fn journal_item(
        sequence: i64,
        timestamp: i64,
        item_id: &str,
        revision: i64,
        body: serde_json::Value,
        turn_item_id: Option<&str>,
    ) -> serde_json::Value {
        let mut row = serde_json::json!({
            "v": 3,
            "epoch": "epoch-1",
            "seq": sequence,
            "fence": 0,
            "ts": timestamp,
            "kind": "item",
            "itemId": item_id,
            "revision": revision,
            "body": body,
        });
        if let Some(turn_item_id) = turn_item_id {
            row["turnScope"] = serde_json::json!({"kind": "turn", "turnItemId": turn_item_id});
        }
        row
    }

    fn tool_call_row(
        sequence: i64,
        item_id: &str,
        body: serde_json::Value,
        turn_item_id: Option<&str>,
    ) -> serde_json::Value {
        journal_item(
            sequence,
            1_767_225_600_000 + sequence * 1_000,
            item_id,
            sequence,
            body,
            turn_item_id,
        )
    }

    fn turn_row(sequence: i64, item_id: &str, state: &str) -> serde_json::Value {
        journal_item(
            sequence,
            1_767_225_600_000 + sequence * 1_000,
            item_id,
            sequence,
            serde_json::json!({"kind": "turn", "turnId": format!("turn-{item_id}"), "state": state}),
            None,
        )
    }

    fn insert_payload(connection: &Connection, sequence: i64, payload: &serde_json::Value) {
        connection
            .execute(
                "INSERT INTO journal_rows VALUES ('session-a', 'epoch-1', ?1, ?2, ?3)",
                rusqlite::params![
                    sequence,
                    1_767_225_600_000 + sequence * 1_000,
                    payload.to_string()
                ],
            )
            .expect("insert journal payload");
    }

    fn insert_payload_for(
        connection: &Connection,
        session_id: &str,
        epoch: &str,
        sequence: i64,
        payload: &serde_json::Value,
    ) {
        connection
            .execute(
                "INSERT INTO journal_rows VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    session_id,
                    epoch,
                    sequence,
                    1_767_225_600_000 + sequence * 1_000,
                    payload.to_string()
                ],
            )
            .expect("insert journal payload for session");
    }

    fn fixture(path: &Path) {
        let connection = Connection::open(path).expect("create journal");
        connection.execute_batch(
            "CREATE TABLE journal_sessions (session_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, epoch TEXT NOT NULL);
             CREATE TABLE journal_rows (session_id TEXT NOT NULL, epoch TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL, row_json TEXT NOT NULL, PRIMARY KEY(session_id, epoch, seq));
             PRAGMA user_version = 4;
             INSERT INTO journal_sessions VALUES ('session-a', 'workspace-a', 'epoch-1');",
        ).expect("create journal fixture");
        let epoch = serde_json::json!({
            "v": 3,
            "epoch": "epoch-1",
            "seq": 1,
            "fence": 0,
            "ts": 1_767_225_601_000i64,
            "kind": "epoch",
            "reason": "session_created",
            "providerHandle": {"kind": "codex", "threadId": "thread-a"}
        });
        insert_payload(&connection, 1, &epoch);
    }

    #[test]
    fn scan_normalizes_orca_structured_tool_call_items() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        insert_payload(
            &source,
            2,
            &tool_call_row(
                2,
                "tool-ok",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "read",

                    "state": "running",
                    "input": null
                }),
                Some("turn-a"),
            ),
        );
        insert_payload(
            &source,
            3,
            &tool_call_row(
                3,
                "tool-ok",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "read",
                    "callId": "call-ok",
                    "state": "completed",
                    "input": {"path": "x"},
                    "output": {
                        "head": "ok",
                        "byteLength": 2,
                        "digest": "sha256-call-ok",
                        "truncated": false
                    }
                }),
                Some("turn-a"),
            ),
        );
        insert_payload(
            &source,
            4,
            &tool_call_row(
                4,
                "tool-fail",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "write",
                    "callId": "call-fail",
                    "state": "failed",
                    "input": {"path": "y"},
                    "output": {
                        "head": "denied",
                        "byteLength": 6,
                        "digest": "sha256-call-fail",
                        "truncated": false
                    }
                }),
                Some("turn-a"),
            ),
        );
        insert_payload(
            &source,
            5,
            &tool_call_row(
                5,
                "tool-interrupted",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "shell",
                    "callId": "call-interrupted",
                    "state": "running",
                    "input": {"command": "sleep"}
                }),
                Some("turn-a"),
            ),
        );
        let unknown = serde_json::json!({
            "v": 3,
            "epoch": "epoch-1",
            "seq": 6,
            "fence": 0,
            "ts": 1_767_225_606_000i64,
            "kind": "item",
            "itemId": "message-a",
            "revision": 1,
            "body": {"kind": "message", "role": "assistant", "blocks": []}
        });
        insert_payload(&source, 6, &unknown);
        insert_payload(
            &source,
            7,
            &tool_call_row(
                7,
                "tool-end-only",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "list",
                    "callId": "call-end-only",
                    "state": "completed",
                    "input": {},
                    "output": {
                        "head": "[]",
                        "byteLength": 2,
                        "digest": "sha256-call-end-only",
                        "truncated": false
                    }
                }),
                None,
            ),
        );
        let mut interrupted_turn = turn_row(8, "turn-a", "interrupted");
        interrupted_turn["body"]["userItemId"] = serde_json::json!("user-a");
        insert_payload(&source, 8, &interrupted_turn);
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(scan_journal(&mut destination, &journal).expect("scan"), 8);
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("repeat scan"),
            0
        );

        let session: (String, Option<i64>) = destination
            .query_row("SELECT provider, run_id FROM sessions", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("normalized session");
        assert_eq!(session, ("orca_journal".to_owned(), None));
        let calls: Vec<NormalizedCall> = {
            let mut statement = destination
                .prepare("SELECT call_id, status, started_at, ended_at, input_hash, output_hash FROM tool_calls ORDER BY call_id")
                .expect("prepare calls");
            statement
                .query_map([], |row| {
                    Ok(NormalizedCall {
                        call_id: row.get(0)?,
                        status: row.get(1)?,
                        started_at: row.get(2)?,
                        ended_at: row.get(3)?,
                        input_hash: row.get(4)?,
                        output_hash: row.get(5)?,
                    })
                })
                .expect("query calls")
                .collect::<Result<_, _>>()
                .expect("collect calls")
        };
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0].call_id, "call-end-only");
        assert_eq!(calls[0].status, "succeeded");
        assert_eq!(calls[0].started_at, None);
        assert_eq!(calls[0].ended_at.as_deref(), Some("1767225607000"));
        assert_eq!(
            calls[0].output_hash.as_deref(),
            Some("sha256-call-end-only")
        );
        assert_eq!(calls[1].call_id, "call-fail");
        assert_eq!(calls[1].status, "failed");
        assert_eq!(calls[1].started_at, None);
        assert_eq!(calls[1].ended_at.as_deref(), Some("1767225604000"));
        assert_eq!(calls[1].input_hash.as_ref().map(String::len), Some(16));
        assert_eq!(calls[1].output_hash.as_deref(), Some("sha256-call-fail"));
        assert_eq!(calls[2].call_id, "call-interrupted");
        assert_eq!(calls[2].status, "interrupted");
        assert_eq!(calls[2].started_at.as_deref(), Some("1767225605000"));
        assert_eq!(calls[2].ended_at.as_deref(), Some("1767225608000"));
        assert_eq!(calls[3].call_id, "call-ok");
        assert_eq!(calls[3].status, "succeeded");
        assert_eq!(calls[3].started_at.as_deref(), Some("1767225602000"));
        assert_eq!(calls[3].ended_at.as_deref(), Some("1767225603000"));
        assert_eq!(calls[3].output_hash.as_deref(), Some("sha256-call-ok"));
        assert_eq!(calls[3].input_hash.as_deref(), Some("587eee3610d4dc68"));

        let linked_run: Option<i64> = destination
            .query_row(
                "SELECT run_id FROM tool_calls WHERE call_id = 'call-interrupted'",
                [],
                |row| row.get(0),
            )
            .expect("explicit turn run link");
        assert!(linked_run.is_some());
        let run: (String, String, Option<String>, i64, Option<i64>, i64) = destination
            .query_row(
                "SELECT t.source_key, r.outcome, r.ended_at,
                        (SELECT COUNT(*) FROM run_sessions WHERE run_id = r.id),
                        r.task_id, r.id
                 FROM runs r JOIN tasks t ON t.id = r.task_id",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("explicit task and run linkage");
        assert_eq!(
            (run.0, run.1, run.2, run.3),
            (
                "11:1:session-a:6:user-a".to_owned(),
                "interrupted".to_owned(),
                Some("1767225608000".to_owned()),
                1
            )
        );
        assert!(run.4.is_some());
        assert_eq!(run.5, linked_run.expect("call run id"));

        let raw_fields: i64 = destination
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tool_calls') WHERE name IN ('args', 'result', 'input', 'output')",
                [],
                |row| row.get(0),
            )
            .expect("normalized raw field count");
        assert_eq!(raw_fields, 0);
        let raw_unknown: i64 = destination
            .query_row(
                "SELECT COUNT(*) FROM raw_events WHERE payload LIKE '%message-a%'",
                [],
                |row| row.get(0),
            )
            .expect("unknown journal row");
        assert_eq!(raw_unknown, 1);
        let sessions: i64 = destination
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .expect("session count");
        assert_eq!(sessions, 1);
    }

    #[test]
    fn lifecycle_batch_order_interrupts_prior_mutations() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        let batch = serde_json::json!({
            "v": 3,
            "epoch": "epoch-1",
            "seq": 2,
            "fence": 0,
            "ts": 1_767_225_602_000i64,
            "kind": "lifecycle-batch",
            "settlementId": "settlement-1",
            "mutations": [
                {
                    "kind": "item",
                    "itemId": "tool-batch",
                    "revision": 1,
                    "body": {
                        "kind": "tool-call",
                        "name": "shell",
                        "callId": "call-batch",
                        "state": "running",
                        "input": {"command": "wait"}
                    },
                    "turnScope": {"kind": "turn", "turnItemId": "turn-batch"}
                },
                {
                    "kind": "item",
                    "itemId": "turn-batch",
                    "revision": 2,
                    "body": {"kind": "turn", "turnId": "turn-batch", "state": "interrupted"}
                }
            ]
        });
        insert_payload(&source, 2, &batch);
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(scan_journal(&mut destination, &journal).expect("scan"), 2);
        let status: String = destination
            .query_row(
                "SELECT status FROM tool_calls WHERE call_id = 'call-batch'",
                [],
                |row| row.get(0),
            )
            .expect("batch tool status");
        assert_eq!(status, "interrupted");
    }

    #[test]
    fn explicit_user_item_links_multiple_turn_runs_to_one_task() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        for (sequence, turn_id, state, user_item_id) in [
            (2, "turn-a", "running", "codex:thread-a:turn:0"),
            (3, "turn-b", "completed", "user-a"),
            (4, "turn-a", "running", "user-a"),
        ] {
            let mut row = turn_row(sequence, turn_id, state);
            row["body"]["userItemId"] = serde_json::json!(user_item_id);
            insert_payload(&source, sequence, &row);
        }
        let mut provider_turn = turn_row(5, "turn-c", "running");
        provider_turn["body"]["userItemId"] = serde_json::json!("turn-c");
        insert_payload(&source, 5, &provider_turn);
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(scan_journal(&mut destination, &journal).expect("scan"), 5);
        let counts: (i64, i64, i64) = destination
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM tasks),
                    (SELECT COUNT(*) FROM runs WHERE task_id = (SELECT id FROM tasks)),
                    (SELECT COUNT(*) FROM run_sessions)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("normalized task, run, and session links");
        assert_eq!(counts, (1, 2, 3));
        let self_keyed_task: Option<i64> = destination
            .query_row(
                "SELECT task_id FROM runs WHERE turn_item_id = 'turn-c'",
                [],
                |row| row.get(0),
            )
            .expect("provider-opened turn");
        assert_eq!(self_keyed_task, None);
        let corrected_tasks: (i64, i64) = destination
            .query_row(
                "SELECT
                    (SELECT task_id FROM runs WHERE turn_item_id = 'turn-a'),
                    (SELECT task_id FROM runs WHERE turn_item_id = 'turn-b')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("corrected request attribution");
        assert_eq!(corrected_tasks.0, corrected_tasks.1);
        let completed_end: Option<String> = destination
            .query_row(
                "SELECT ended_at FROM runs WHERE turn_item_id = 'turn-b'",
                [],
                |row| row.get(0),
            )
            .expect("completed turn end time");
        assert_eq!(completed_end.as_deref(), Some("1767225603000"));
    }

    #[test]
    fn explicit_terminal_tool_revision_supersedes_turn_interruption() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        insert_payload(
            &source,
            2,
            &tool_call_row(
                2,
                "tool-revised",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "shell",
                    "callId": "call-revised",
                    "state": "running",
                    "input": {"command": "work"}
                }),
                Some("turn-a"),
            ),
        );
        insert_payload(&source, 3, &turn_row(3, "turn-a", "interrupted"));
        insert_payload(
            &source,
            4,
            &tool_call_row(
                4,
                "tool-revised",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "shell",
                    "callId": "call-revised",
                    "state": "failed",
                    "input": {"command": "work"}
                }),
                Some("turn-a"),
            ),
        );
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(scan_journal(&mut destination, &journal).expect("scan"), 4);
        let status: String = destination
            .query_row(
                "SELECT status FROM tool_calls WHERE call_id = 'call-revised'",
                [],
                |row| row.get(0),
            )
            .expect("revised terminal status");
        assert_eq!(status, "failed");
    }

    #[test]
    fn interrupted_turn_marks_unfinished_calls_interrupted() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        insert_payload(
            &source,
            2,
            &tool_call_row(
                2,
                "tool-open",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "shell",
                    "callId": "call-interrupted",
                    "state": "running",
                    "input": {"command": "sleep"}
                }),
                Some("turn-a"),
            ),
        );
        insert_payload(&source, 3, &turn_row(3, "turn-a", "interrupted"));
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(scan_journal(&mut destination, &journal).expect("scan"), 3);
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("repeat scan"),
            0
        );
        let call: (String, String, Option<String>, Option<i64>, Option<i64>) = destination
            .query_row(
                "SELECT call_id, status, ended_at, start_event_id, end_event_id FROM tool_calls",
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
            .expect("interrupted tool call");
        assert_eq!(call.0, "call-interrupted");
        assert_eq!(call.1, "interrupted");
        assert_eq!(call.2.as_deref(), Some("1767225603000"));
        assert!(call.3.is_some());
        assert!(call.4.is_some());
    }

    #[test]
    fn replay_does_not_interrupt_a_call_created_after_the_turn_ended() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        insert_payload(&source, 2, &turn_row(2, "turn-a", "interrupted"));
        insert_payload(
            &source,
            3,
            &tool_call_row(
                3,
                "tool-after",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "read",
                    "callId": "call-after",
                    "state": "running",
                    "input": {"path": "x"}
                }),
                Some("turn-a"),
            ),
        );
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(scan_journal(&mut destination, &journal).expect("scan"), 3);
        let status: String = destination
            .query_row(
                "SELECT status FROM tool_calls WHERE call_id = 'call-after'",
                [],
                |row| row.get(0),
            )
            .expect("call status");
        assert_eq!(status, "started");
    }

    #[test]
    fn interrupted_turn_in_another_journal_does_not_close_a_call() {
        let directory = tempdir().expect("temporary directory");
        let first_journal = directory.path().join("first").join(JOURNAL_NAME);
        let second_journal = directory.path().join("second").join(JOURNAL_NAME);
        std::fs::create_dir_all(first_journal.parent().expect("first journal directory"))
            .expect("create first journal directory");
        std::fs::create_dir_all(second_journal.parent().expect("second journal directory"))
            .expect("create second journal directory");
        fixture(&first_journal);
        fixture(&second_journal);
        let first = Connection::open(&first_journal).expect("open first journal");
        insert_payload(
            &first,
            2,
            &tool_call_row(
                2,
                "tool-open",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "read",
                    "callId": "call-open",
                    "state": "running",
                    "input": {"path": "x"}
                }),
                Some("turn-a"),
            ),
        );
        let second = Connection::open(&second_journal).expect("open second journal");
        insert_payload(&second, 2, &turn_row(2, "turn-a", "interrupted"));
        drop(first);
        drop(second);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");

        assert_eq!(
            scan_journal(&mut destination, &first_journal).expect("scan first"),
            2
        );
        assert_eq!(
            scan_journal(&mut destination, &second_journal).expect("scan second"),
            2
        );
        let status: String = destination
            .query_row(
                "SELECT status FROM tool_calls WHERE call_id = 'call-open'",
                [],
                |row| row.get(0),
            )
            .expect("call status");
        assert_eq!(status, "started");
        let separate_sources: (i64, i64) = destination
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sessions), (SELECT COUNT(*) FROM runs)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("source-qualified sessions and runs");
        assert_eq!(separate_sources, (2, 2));
    }

    #[test]
    fn interrupted_turn_from_a_replacement_epoch_closes_an_older_call() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let source = Connection::open(&journal).expect("open journal");
        insert_payload(
            &source,
            2,
            &tool_call_row(
                2,
                "tool-open",
                serde_json::json!({
                    "kind": "tool-call",
                    "name": "read",
                    "callId": "call-open",
                    "state": "running",
                    "input": {"path": "x"}
                }),
                Some("turn-a"),
            ),
        );
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("scan first epoch"),
            2
        );
        let replacement_turn = serde_json::json!({
            "v": 3,
            "epoch": "epoch-2",
            "seq": 1,
            "fence": 0,
            "ts": 1_767_225_602_000i64,
            "kind": "item",
            "itemId": "turn-a",
            "revision": 1,
            "body": {"kind": "turn", "turnId": "turn-a", "state": "interrupted"}
        });
        Connection::open(&journal)
            .expect("reopen journal")
            .execute_batch(
                "UPDATE journal_sessions SET epoch = 'epoch-2';
                 DELETE FROM journal_rows WHERE epoch = 'epoch-1';",
            )
            .expect("replace published epoch");
        let source = Connection::open(&journal).expect("open replacement journal");
        source
            .execute(
                "INSERT INTO journal_rows VALUES ('session-a', 'epoch-2', 1, 1767225602000, ?1)",
                [replacement_turn.to_string()],
            )
            .expect("add replacement turn row");
        destination
            .execute_batch(
                "DELETE FROM tool_calls;
                 DELETE FROM run_sessions;
                 DELETE FROM runs;
                 DELETE FROM sessions;
                 DELETE FROM tasks;
                 UPDATE scan_checkpoints
                 SET cursor = '{\"sessions\":{\"session-a\":{\"epoch\":\"epoch-1\",\"seq\":2}},\"normalization_backfill_complete\":false}';",
            )
            .expect("simulate an unmigrated normalization projection");

        assert_eq!(
            scan_journal(&mut destination, &journal).expect("scan second epoch"),
            1
        );
        let status: String = destination
            .query_row(
                "SELECT status FROM tool_calls WHERE call_id = 'call-open'",
                [],
                |row| row.get(0),
            )
            .expect("call status");
        assert_eq!(status, "interrupted");
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

    #[test]
    fn replays_legacy_raw_events_with_a_colon_in_session_id() {
        let directory = tempdir().expect("temporary directory");
        let journal = directory.path().join(JOURNAL_NAME);
        fixture(&journal);
        let session_id = "thread:claude:alpha";
        Connection::open(&journal)
            .expect("open journal")
            .execute_batch(&format!(
                "INSERT INTO journal_sessions VALUES ('{session_id}', 'workspace-a', 'epoch-1');
                 DELETE FROM journal_sessions WHERE session_id = 'session-a';",
            ))
            .expect("switch journal session");
        let source = Connection::open(&journal).expect("open journal writer");
        insert_payload_for(
            &source,
            session_id,
            "epoch-1",
            2,
            &serde_json::json!({
                "v": 3,
                "epoch": "epoch-1",
                "seq": 2,
                "fence": 0,
                "ts": 1_767_225_602_000i64,
                "kind": "item",
                "itemId": "tool-colon",
                "revision": 2,
                "body": {
                    "kind": "tool-call",
                    "name": "read",
                    "callId": "call-colon",
                    "state": "completed",
                    "input": {"path": "x"},
                    "output": {
                        "head": "ok",
                        "byteLength": 2,
                        "digest": "sha256-call-colon",
                        "truncated": false
                    }
                }
            }),
        );
        drop(source);
        let mut destination =
            crate::storage::open(directory.path().join("orcalens.db")).expect("open destination");
        assert_eq!(
            scan_journal(&mut destination, &journal).expect("first scan"),
            1
        );
        let call: (String, String) = destination
            .query_row(
                "SELECT status, session.provider_session_id
                 FROM tool_calls JOIN sessions session ON session.id = tool_calls.session_id
                 WHERE call_id = 'call-colon'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("colon-bearing session call");
        assert_eq!(call.0, "succeeded");
        let provider_prefix = destination
            .query_row("SELECT id FROM sources", [], |row| row.get::<_, i64>(0))
            .expect("source id");
        assert_eq!(call.1, format!("{provider_prefix}:{session_id}"));
    }
}

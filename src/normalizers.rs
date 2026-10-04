use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value;

#[derive(Clone, Copy)]
pub(crate) struct JournalSourceRow<'a> {
    pub(crate) source_id: i64,
    pub(crate) provider_session_id: &'a str,
    pub(crate) epoch: &'a str,
    pub(crate) sequence: i64,
    pub(crate) timestamp: i64,
    pub(crate) raw_event_id: i64,
    pub(crate) mutation_index: i64,
}

pub(crate) fn normalize_journal_row(
    transaction: &Transaction<'_>,
    source: JournalSourceRow<'_>,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let row: Value = serde_json::from_str(payload)?;
    match row.get("kind").and_then(Value::as_str) {
        Some("item") => normalize_item(
            transaction,
            &source,
            row.get("itemId").and_then(Value::as_str),
            row.get("body"),
            row.get("turnScope"),
        )?,
        Some("lifecycle-batch") => {
            if let Some(mutations) = row.get("mutations").and_then(Value::as_array) {
                for (mutation_index, mutation) in mutations.iter().enumerate() {
                    if mutation.get("kind").and_then(Value::as_str) == Some("item") {
                        let source = JournalSourceRow {
                            mutation_index: mutation_index as i64,
                            ..source
                        };
                        normalize_item(
                            transaction,
                            &source,
                            mutation.get("itemId").and_then(Value::as_str),
                            mutation.get("body"),
                            mutation.get("turnScope"),
                        )?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn normalize_item(
    transaction: &Transaction<'_>,
    source: &JournalSourceRow<'_>,
    item_id: Option<&str>,
    body: Option<&Value>,
    turn_scope: Option<&Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(item_id) = item_id.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let Some(body) = body else {
        return Ok(());
    };
    let body_kind = body.get("kind").and_then(Value::as_str);
    let is_tool_call = body_kind == Some("tool-call");
    let is_turn = body_kind == Some("turn");
    if !is_tool_call && !is_turn {
        return Ok(());
    }
    let provider_session_id = format!("{}:{}", source.source_id, source.provider_session_id);
    let session_id: i64 = transaction.query_row(
        "INSERT INTO sessions (run_id, provider, provider_session_id)
         VALUES (NULL, 'orca_journal', ?1)
         ON CONFLICT (provider, provider_session_id) DO UPDATE SET provider_session_id = excluded.provider_session_id
         RETURNING id",
        [&provider_session_id],
        |row| row.get(0),
    )?;
    if is_turn {
        ensure_run(
            transaction,
            &provider_session_id,
            session_id,
            item_id,
            Some(body),
            source.timestamp,
            source.raw_event_id,
        )?;
        if body.get("state").and_then(Value::as_str) == Some("interrupted") {
            transaction.execute(
                "UPDATE tool_calls
                 SET ended_at = ?1, status = 'interrupted', end_event_id = ?2
                 WHERE session_id = ?3 AND turn_item_id = ?4 AND status = 'started'
                   AND start_event_id IS NOT NULL
                   AND (start_epoch <> ?5 OR start_seq < ?6
                        OR (start_seq = ?6 AND start_index < ?7))
                   AND (SELECT source_id FROM raw_events WHERE id = tool_calls.start_event_id)
                       = (SELECT source_id FROM raw_events WHERE id = ?2)",
                rusqlite::params![
                    source.timestamp.to_string(),
                    source.raw_event_id,
                    session_id,
                    item_id,
                    source.epoch,
                    source.sequence,
                    source.mutation_index
                ],
            )?;
        }
        return Ok(());
    }
    let call_id = body
        .get("callId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let Some(tool_name) = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let state = match body.get("state").and_then(Value::as_str) {
        Some("running") => "started",
        Some("completed") => "succeeded",
        Some("failed") => "failed",
        _ => return Ok(()),
    };
    let turn_item_id = turn_scope
        .filter(|scope| scope.get("kind").and_then(Value::as_str) == Some("turn"))
        .and_then(|scope| scope.get("turnItemId"))
        .and_then(Value::as_str);
    let run_id = turn_item_id
        .map(|turn_item_id| {
            ensure_run(
                transaction,
                &provider_session_id,
                session_id,
                turn_item_id,
                None,
                source.timestamp,
                source.raw_event_id,
            )
        })
        .transpose()?;
    let input_hash = body
        .get("input")
        .filter(|input| !input.is_null())
        .map(serde_json::to_string)
        .transpose()?
        .map(|input| payload_hash(input.as_bytes()));
    let output_hash = body
        .get("output")
        .and_then(|output| output.get("digest"))
        .and_then(Value::as_str);
    let normalization_key = format!("{session_id}:item:{}:{item_id}", item_id.len());
    let timestamp = source.timestamp.to_string();
    if state == "started" {
        transaction.execute(
            "INSERT INTO tool_calls (run_id, normalization_key, session_id, call_id, tool_name, started_at, status, input_hash, start_event_id, turn_item_id, start_epoch, start_seq, start_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'started', ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (normalization_key) DO UPDATE SET
                 run_id = COALESCE(tool_calls.run_id, excluded.run_id),
                 call_id = COALESCE(excluded.call_id, tool_calls.call_id),
                 tool_name = excluded.tool_name,
                 started_at = COALESCE(tool_calls.started_at, excluded.started_at),
                 status = CASE WHEN tool_calls.ended_at IS NULL THEN 'started' ELSE tool_calls.status END,
                 input_hash = COALESCE(excluded.input_hash, tool_calls.input_hash),
                 start_event_id = COALESCE(tool_calls.start_event_id, excluded.start_event_id),
                 turn_item_id = COALESCE(tool_calls.turn_item_id, excluded.turn_item_id),
                 start_epoch = COALESCE(tool_calls.start_epoch, excluded.start_epoch),
                 start_seq = COALESCE(tool_calls.start_seq, excluded.start_seq),
                 start_index = COALESCE(tool_calls.start_index, excluded.start_index)",
            rusqlite::params![run_id, normalization_key, session_id, call_id, tool_name, timestamp, input_hash, source.raw_event_id, turn_item_id, source.epoch, source.sequence, source.mutation_index],
        )?;
    } else {
        transaction.execute(
            "INSERT INTO tool_calls (run_id, normalization_key, session_id, call_id, tool_name, ended_at, status, input_hash, output_hash, end_event_id, turn_item_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (normalization_key) DO UPDATE SET
                 run_id = COALESCE(tool_calls.run_id, excluded.run_id),
                 call_id = COALESCE(excluded.call_id, tool_calls.call_id),
                 tool_name = excluded.tool_name,
                 ended_at = CASE WHEN tool_calls.status = 'interrupted' THEN excluded.ended_at ELSE COALESCE(tool_calls.ended_at, excluded.ended_at) END,
                 status = CASE WHEN tool_calls.status = 'interrupted' OR tool_calls.ended_at IS NULL THEN excluded.status ELSE tool_calls.status END,
                 input_hash = COALESCE(excluded.input_hash, tool_calls.input_hash),
                 output_hash = COALESCE(tool_calls.output_hash, excluded.output_hash),
                 end_event_id = CASE WHEN tool_calls.status = 'interrupted' THEN excluded.end_event_id ELSE COALESCE(tool_calls.end_event_id, excluded.end_event_id) END,
                 turn_item_id = COALESCE(tool_calls.turn_item_id, excluded.turn_item_id)",
            rusqlite::params![run_id, normalization_key, session_id, call_id, tool_name, timestamp, state, input_hash, output_hash, source.raw_event_id, turn_item_id],
        )?;
    }
    Ok(())
}

fn ensure_run(
    transaction: &Transaction<'_>,
    provider_session_id: &str,
    session_id: i64,
    turn_item_id: &str,
    turn_body: Option<&Value>,
    source_timestamp: i64,
    raw_event_id: i64,
) -> Result<i64, Box<dyn std::error::Error>> {
    let task_id: Option<i64> = turn_body
        .and_then(|body| body.get("userItemId"))
        .and_then(Value::as_str)
        .filter(|user_item_id| !user_item_id.is_empty() && *user_item_id != turn_item_id)
        .map(|user_item_id| {
            let source_key = format!(
                "{}:{}:{}:{}",
                provider_session_id.len(),
                provider_session_id,
                user_item_id.len(),
                user_item_id
            );
            transaction.query_row(
                "INSERT INTO tasks (source, source_key)
                 VALUES ('orca_journal', ?1)
                 ON CONFLICT (source, source_key) DO UPDATE SET source_key = excluded.source_key
                 RETURNING id",
                [source_key],
                |row| row.get(0),
            )
        })
        .transpose()?;
    let state = turn_body
        .and_then(|body| body.get("state"))
        .and_then(Value::as_str);
    let started_at = turn_body
        .and_then(|body| body.get("startedAt"))
        .and_then(Value::as_i64)
        .or_else(|| (state == Some("running")).then_some(source_timestamp))
        .map(|timestamp| timestamp.to_string());
    let ended_at = match state {
        Some("completed" | "interrupted") => Some(
            turn_body
                .and_then(|body| body.get("completedAt"))
                .and_then(Value::as_i64)
                .unwrap_or(source_timestamp)
                .to_string(),
        ),
        _ => None,
    };
    let outcome = turn_body
        .and_then(|body| body.get("outcome"))
        .and_then(Value::as_str)
        .or(match state {
            Some(outcome @ ("interrupted" | "unverifiable")) => Some(outcome),
            _ => None,
        });
    let normalization_key = format!("{session_id}:turn:{}:{turn_item_id}", turn_item_id.len());
    let start_event_id = started_at.as_ref().map(|_| raw_event_id);
    let end_event_id = ended_at.as_ref().map(|_| raw_event_id);
    let previous_task_id: Option<i64> = transaction
        .query_row(
            "SELECT task_id FROM runs WHERE normalization_key = ?1",
            [&normalization_key],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let run_id: i64 = transaction.query_row(
        "INSERT INTO runs (task_id, started_at, ended_at, outcome, normalization_key, turn_item_id, start_event_id, end_event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (normalization_key) DO UPDATE SET
             task_id = COALESCE(excluded.task_id, runs.task_id),
             started_at = COALESCE(runs.started_at, excluded.started_at),
             ended_at = COALESCE(runs.ended_at, excluded.ended_at),
             outcome = COALESCE(excluded.outcome, runs.outcome),
             start_event_id = COALESCE(runs.start_event_id, excluded.start_event_id),
             end_event_id = COALESCE(runs.end_event_id, excluded.end_event_id)
         RETURNING id",
        rusqlite::params![
            task_id,
            started_at,
            ended_at,
            outcome,
            normalization_key,
            turn_item_id,
            start_event_id,
            end_event_id
        ],
        |row| row.get(0),
    )?;
    if let (Some(previous_task_id), Some(task_id)) = (previous_task_id, task_id) {
        if previous_task_id != task_id {
            transaction.execute(
                "DELETE FROM tasks
                 WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM runs WHERE task_id = ?1)",
                [previous_task_id],
            )?;
        }
    }
    transaction.execute(
        "INSERT INTO run_sessions (run_id, session_id, source_event_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (run_id, session_id) DO UPDATE SET
             source_event_id = COALESCE(run_sessions.source_event_id, excluded.source_event_id)",
        rusqlite::params![run_id, session_id, raw_event_id],
    )?;
    Ok(run_id)
}

fn payload_hash(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::payload_hash;

    #[test]
    fn payload_hash_uses_fnv1a_64() {
        assert_eq!(payload_hash(b"hello"), "a430d84680aabd0b");
    }
}

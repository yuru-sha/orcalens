use crate::analyzers::{Finding, WasteThresholds};
use crate::cli::Command;
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::{
    error::Error,
    io::{self, Write},
    path::Path,
    time::Duration,
};
const MAX_RAW_EVENT_PREVIEW_CHARS: i64 = 250_000;

#[derive(Serialize)]
pub struct CommandOutput {
    #[serde(serialize_with = "serialize_command")]
    pub command: Command,
    pub data: CommandData,
}

fn serialize_command<S: serde::Serializer>(
    command: &Command,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match command {
        Command::Scan => "scan",
        Command::Runs => "runs",
        Command::Skills { .. } => "skills",
        Command::Waste { .. } => "waste",
        Command::Dashboard => "dashboard",
        Command::Report => "report",
    })
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum CommandData {
    Scan(ScanResult),
    Skills(SkillsResult),
    Waste(WasteResult),
    List(ListResult),
    Report(ReportResult),
}

#[derive(Clone, Debug, Serialize)]
pub struct ScanResult {
    pub sources_scanned: u64,
    pub events_imported: u64,
    pub collectors_available: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ListResult {
    pub items: Vec<RunResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunResult {
    pub id: i64,
    pub task_id: Option<i64>,
    pub task_title: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub outcome: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub sessions: Vec<i64>,
    pub tool_calls: i64,
    pub skill_calls: i64,
    pub evidence: Vec<i64>,
}
type RunRowMetadata = (
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Clone, Debug, Serialize)]
pub struct TaskResult {
    pub id: i64,
    pub source: String,
    pub source_key: String,
    pub title: Option<String>,
    pub runs: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskDetail {
    pub task: TaskResult,
    pub runs: Vec<RunResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCallResult {
    pub id: i64,
    pub run_id: Option<i64>,
    pub session_id: Option<i64>,
    pub tool: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub status: Option<String>,
    pub evidence: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillCallResult {
    pub id: i64,
    pub run_id: Option<i64>,
    pub session_id: Option<i64>,
    pub skill: String,
    pub provider: String,
    pub started_at: Option<String>,
    pub evidence: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunDetail {
    pub run: RunResult,
    pub tool_calls: Vec<ToolCallResult>,
    pub skill_calls: Vec<SkillCallResult>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ToolResult {
    pub name: String,
    pub calls: i64,
    pub failed_calls: i64,
    pub runs: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillResult {
    pub name: String,
    pub installed: bool,
    pub providers: Vec<String>,
    pub calls: i64,
    pub runs: i64,
    pub last_used: Option<String>,
    pub inactive: bool,
    pub status: SkillStatus,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatus {
    Active,
    Dormant,
    NeverUsed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SkillCell {
    pub run_id: i64,
    pub skill: String,
    pub calls: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AgentModelResult {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub runs: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RawEventResult {
    pub id: i64,
    pub source_type: String,
    pub source_path: String,
    pub source_identity: String,
    pub source_timestamp: Option<String>,
    pub observed_at: String,
    pub payload_hash: String,
    pub payload_bytes: i64,
    pub payload_truncated: bool,
    pub payload: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct InventoryResult {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum EvidenceTarget {
    Run(i64),
    ToolCall(i64),
    SkillCall(i64),
    Inventory(i64),
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "record", rename_all = "snake_case")]
pub enum EvidenceRecord {
    Run(RunResult),
    ToolCall(ToolCallResult),
    SkillCall(SkillCallResult),
    Inventory(InventoryResult),
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceDetail {
    pub record: EvidenceRecord,
    pub source_events: Vec<RawEventResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillsResult {
    pub items: Vec<SkillResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WasteResult {
    pub items: Vec<Finding>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportResult {
    pub runs: i64,
    pub tasks: i64,
    pub skills: i64,
    pub tool_calls: i64,
    pub skill_calls: i64,
    pub findings: i64,
    pub computed_findings: usize,
    pub durations: DurationHistogram,
    pub skill_status: SkillStatusCounts,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DurationHistogram {
    pub under_one_minute: i64,
    pub one_to_five_minutes: i64,
    pub five_to_fifteen_minutes: i64,
    pub fifteen_to_sixty_minutes: i64,
    pub over_sixty_minutes: i64,
    pub unknown: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentSkillCell {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub skill: String,
    pub calls: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SkillStatusCounts {
    pub active: i64,
    pub dormant: i64,
    pub never_used: i64,
    pub unknown: i64,
}

#[derive(Clone, Debug)]
pub enum ReportView {
    Runs,
    Tasks,
    Task(i64),
    Run(i64),
    Skills,
    SkillMatrix,
    SkillCalls {
        run_id: Option<i64>,
        skill: Option<String>,
    },
    Tools,
    Tool(String),
    AgentModels,
    AgentSkillMatrix,
    Waste,
    Evidence(EvidenceTarget),
    SourceEvent(i64),
    Summary,
}

#[derive(Clone, Debug)]
pub struct ReportRequest {
    pub view: ReportView,
    pub limit: usize,
    pub offset: usize,
    pub inactivity_days: u64,
    pub thresholds: WasteThresholds,
}

impl ReportRequest {
    pub fn new(view: ReportView) -> Self {
        Self {
            view,
            limit: 100,
            offset: 0,
            inactivity_days: 90,
            thresholds: WasteThresholds::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum ReportData {
    Runs(ListResult),
    Tasks(Vec<TaskResult>),
    Task(TaskDetail),
    Run(RunDetail),
    Skills(SkillsResult),
    SkillMatrix(Vec<SkillCell>),
    SkillCalls(Vec<SkillCallResult>),
    Tools(Vec<ToolResult>),
    Tool(Vec<ToolCallResult>),
    AgentModels(Vec<AgentModelResult>),
    AgentSkillMatrix(Vec<AgentSkillCell>),
    Waste(WasteResult),
    Evidence(EvidenceDetail),
    SourceEvent(RawEventResult),
    Summary(ReportResult),
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportSnapshot {
    pub view: ReportData,
    pub metadata: ReportMetadata,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportMetadata {
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

pub struct ReportReader {
    connection: Connection,
}

impl ReportReader {
    pub fn open_default_read_only() -> Result<Self, Box<dyn Error>> {
        Self::open_read_only(crate::storage::default_database_path()?)
    }

    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.pragma_update(None, "query_only", "ON")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: Option<i64> =
            connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })?;
        if version != Some(4) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "analytics database schema is not ready",
            )
            .into());
        }
        Ok(Self { connection })
    }

    pub fn query(&self, request: &ReportRequest) -> Result<ReportSnapshot, Box<dyn Error>> {
        query(&self.connection, request)
    }
}

pub fn scan(connection: &mut Connection) -> Result<ScanResult, Box<dyn Error>> {
    let summary = crate::collectors::scan(connection)?;
    Ok(ScanResult {
        sources_scanned: summary.sources_scanned,
        events_imported: summary.events_imported,
        collectors_available: 1,
    })
}

pub fn refresh_skills(
    connection: &mut Connection,
    inactivity_days: u64,
) -> Result<SkillsResult, Box<dyn Error>> {
    crate::skills::refresh(connection, inactivity_days)?;
    Ok(SkillsResult {
        items: skill_rows(connection, inactivity_days)?,
    })
}

pub fn query(
    connection: &Connection,
    request: &ReportRequest,
) -> Result<ReportSnapshot, Box<dyn Error>> {
    let transaction = connection.unchecked_transaction()?;
    let snapshot = query_snapshot(&transaction, request)?;
    transaction.commit()?;
    Ok(snapshot)
}

fn query_snapshot(
    connection: &Connection,
    request: &ReportRequest,
) -> Result<ReportSnapshot, Box<dyn Error>> {
    let limit = request.limit.clamp(1, 500);
    let offset = request.offset.min(i64::MAX as usize);
    let (view, total) = match &request.view {
        ReportView::Runs => {
            let total = count(connection, "runs")?;
            (
                ReportData::Runs(ListResult {
                    items: run_rows(connection, limit, offset)?,
                }),
                total,
            )
        }
        ReportView::Tasks => {
            let total = count(connection, "tasks")?;
            (
                ReportData::Tasks(task_rows(connection, limit, offset)?),
                total,
            )
        }
        ReportView::Task(id) => {
            let task = task_detail(connection, *id, limit, offset)?;
            let total = task.task.runs;
            (ReportData::Task(task), total)
        }
        ReportView::Run(id) => (
            ReportData::Run(run_detail(connection, *id, limit, offset)?),
            1,
        ),
        ReportView::Skills => {
            let items = skill_rows(connection, request.inactivity_days)?;
            let total = items.len() as i64;
            (
                ReportData::Skills(SkillsResult {
                    items: page(items, limit, offset),
                }),
                total,
            )
        }
        ReportView::SkillMatrix => {
            let total: i64 = connection.query_row(
                "SELECT COUNT(*) FROM (SELECT run_id, skill_name FROM skill_calls WHERE run_id IS NOT NULL GROUP BY run_id, skill_name)",
                [],
                |row| row.get(0),
            )?;
            (
                ReportData::SkillMatrix(skill_matrix(connection, limit, offset)?),
                total,
            )
        }
        ReportView::SkillCalls { run_id, skill } => {
            let (items, total) =
                filtered_skill_calls(connection, *run_id, skill.as_deref(), limit, offset)?;
            (ReportData::SkillCalls(items), total)
        }
        ReportView::Tools => {
            let total: i64 = connection.query_row(
                "SELECT COUNT(DISTINCT tool_name) FROM tool_calls",
                [],
                |row| row.get(0),
            )?;
            (
                ReportData::Tools(tool_rows(connection, limit, offset)?),
                total,
            )
        }
        ReportView::Tool(name) => {
            let total: i64 = connection.query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE tool_name = ?1",
                [name],
                |row| row.get(0),
            )?;
            (
                ReportData::Tool(tool_detail(connection, name, limit, offset)?),
                total,
            )
        }
        ReportView::AgentModels => {
            let total: i64 = connection.query_row(
                "SELECT COUNT(*) FROM (SELECT agent, model FROM runs WHERE agent IS NOT NULL OR model IS NOT NULL GROUP BY agent, model)",
                [],
                |row| row.get(0),
            )?;
            (
                ReportData::AgentModels(agent_models(connection, limit, offset)?),
                total,
            )
        }
        ReportView::AgentSkillMatrix => {
            let total: i64 = connection.query_row(
                "SELECT COUNT(*) FROM (SELECT r.agent, r.model, s.skill_name FROM runs r JOIN skill_calls s ON s.run_id = r.id WHERE r.agent IS NOT NULL OR r.model IS NOT NULL GROUP BY r.agent, r.model, s.skill_name)",
                [],
                |row| row.get(0),
            )?;
            (
                ReportData::AgentSkillMatrix(agent_skill_matrix(connection, limit, offset)?),
                total,
            )
        }
        ReportView::Waste => {
            let items = crate::analyzers::analyze(connection, request.thresholds)?;
            let total = items.len() as i64;
            (
                ReportData::Waste(WasteResult {
                    items: page(items, limit, offset),
                }),
                total,
            )
        }
        ReportView::Evidence(target) => (
            ReportData::Evidence(evidence_detail(connection, *target)?),
            1,
        ),
        ReportView::SourceEvent(id) => (ReportData::SourceEvent(raw_event(connection, *id)?), 1),
        ReportView::Summary => (
            ReportData::Summary(summary(
                connection,
                request.inactivity_days,
                request.thresholds,
            )?),
            count(connection, "runs")?,
        ),
    };
    Ok(ReportSnapshot {
        view,
        metadata: ReportMetadata {
            total,
            limit,
            offset,
        },
    })
}

fn page<T>(items: Vec<T>, limit: usize, offset: usize) -> Vec<T> {
    items.into_iter().skip(offset).take(limit).collect()
}

fn run_rows(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<RunResult>, Box<dyn Error>> {
    let ids = {
        let mut statement =
            connection.prepare("SELECT id FROM runs ORDER BY id LIMIT ?1 OFFSET ?2")?;
        let rows = statement
            .query_map(params![limit as i64, offset as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    ids.into_iter().map(|id| run_row(connection, id)).collect()
}

fn run_row(connection: &Connection, id: i64) -> Result<RunResult, Box<dyn Error>> {
    let (task_id, task_title, started_at, ended_at, outcome, agent, model): RunRowMetadata = connection.query_row(
        "SELECT r.task_id, t.title, r.started_at, r.ended_at, r.outcome, r.agent, r.model FROM runs r LEFT JOIN tasks t ON t.id = r.task_id WHERE r.id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
    )?;
    let sessions = {
        let mut statement = connection.prepare("SELECT id FROM sessions WHERE run_id = ?1 UNION SELECT session_id FROM run_sessions WHERE run_id = ?1 ORDER BY 1")?;
        let rows = statement
            .query_map([id], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        rows
    };
    let tool_calls: i64 = connection.query_row(
        "SELECT COUNT(*) FROM tool_calls WHERE run_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let skill_calls: i64 = connection.query_row(
        "SELECT COUNT(*) FROM skill_calls WHERE run_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let mut evidence = Vec::new();
    for sql in [
        "SELECT start_event_id FROM runs WHERE id = ?1 UNION SELECT end_event_id FROM runs WHERE id = ?1",
        "SELECT start_event_id FROM tool_calls WHERE run_id = ?1 UNION SELECT end_event_id FROM tool_calls WHERE run_id = ?1",
        "SELECT source_event_id FROM run_sessions WHERE run_id = ?1",
        "SELECT source_event_id FROM skill_calls WHERE run_id = ?1",
    ] {
        let mut statement = connection.prepare(sql)?;
        evidence.extend(statement.query_map([id], |row| row.get::<_, Option<i64>>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().flatten());
    }
    evidence.sort_unstable();
    evidence.dedup();
    Ok(RunResult {
        id,
        task_id,
        task_title,
        duration_ms: duration_ms(started_at.as_deref(), ended_at.as_deref()),
        started_at,
        ended_at,
        outcome,
        agent,
        model,
        sessions,
        tool_calls,
        skill_calls,
        evidence,
    })
}

fn task_rows(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<TaskResult>, Box<dyn Error>> {
    let mut statement = connection.prepare("SELECT t.id, t.source, t.source_key, t.title, COUNT(r.id) FROM tasks t LEFT JOIN runs r ON r.task_id = t.id GROUP BY t.id ORDER BY t.id LIMIT ?1 OFFSET ?2")?;
    let rows = statement
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(TaskResult {
                id: row.get(0)?,
                source: row.get(1)?,
                source_key: row.get(2)?,
                title: row.get(3)?,
                runs: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn task_detail(
    connection: &Connection,
    id: i64,
    limit: usize,
    offset: usize,
) -> Result<TaskDetail, Box<dyn Error>> {
    let task = connection.query_row("SELECT t.id, t.source, t.source_key, t.title, COUNT(r.id) FROM tasks t LEFT JOIN runs r ON r.task_id = t.id WHERE t.id = ?1 GROUP BY t.id", [id], |row| Ok(TaskResult { id: row.get(0)?, source: row.get(1)?, source_key: row.get(2)?, title: row.get(3)?, runs: row.get(4)? }))?;
    let mut statement = connection
        .prepare("SELECT id FROM runs WHERE task_id = ?1 ORDER BY id LIMIT ?2 OFFSET ?3")?;
    let ids = statement
        .query_map(params![id, limit as i64, offset as i64], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let runs = ids
        .into_iter()
        .map(|run_id| run_row(connection, run_id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TaskDetail { task, runs })
}

fn run_detail(
    connection: &Connection,
    id: i64,
    limit: usize,
    offset: usize,
) -> Result<RunDetail, Box<dyn Error>> {
    let run = run_row(connection, id)?;
    let tool_calls = tool_calls_for_run(connection, id, limit, offset)?;
    let skill_calls = skill_calls_for_run(connection, id, limit, offset)?;
    Ok(RunDetail {
        run,
        tool_calls,
        skill_calls,
    })
}

fn tool_calls_for_run(
    connection: &Connection,
    run_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<ToolCallResult>, Box<dyn Error>> {
    let mut statement = connection
        .prepare("SELECT id FROM tool_calls WHERE run_id = ?1 ORDER BY id LIMIT ?2 OFFSET ?3")?;
    let ids = statement
        .query_map(params![run_id, limit as i64, offset as i64], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| tool_call(connection, id))
        .collect()
}

fn skill_calls_for_run(
    connection: &Connection,
    run_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<SkillCallResult>, Box<dyn Error>> {
    let mut statement = connection
        .prepare("SELECT id FROM skill_calls WHERE run_id = ?1 ORDER BY id LIMIT ?2 OFFSET ?3")?;
    let ids = statement
        .query_map(params![run_id, limit as i64, offset as i64], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| skill_call(connection, id))
        .collect()
}

fn tool_call(connection: &Connection, id: i64) -> Result<ToolCallResult, Box<dyn Error>> {
    let (run_id, session_id, tool, started_at, ended_at, status, start, end) = connection.query_row("SELECT run_id, session_id, tool_name, started_at, ended_at, status, start_event_id, end_event_id FROM tool_calls WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get::<_, Option<i64>>(6)?, row.get::<_, Option<i64>>(7)?)))?;
    let evidence = [start, end]
        .into_iter()
        .flatten()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ToolCallResult {
        id,
        run_id,
        session_id,
        tool,
        started_at,
        ended_at,
        status,
        evidence,
    })
}

fn skill_call(connection: &Connection, id: i64) -> Result<SkillCallResult, Box<dyn Error>> {
    let (run_id, session_id, skill, provider, started_at, event) = connection.query_row("SELECT run_id, session_id, skill_name, source, started_at, source_event_id FROM skill_calls WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get::<_, Option<i64>>(5)?)))?;
    Ok(SkillCallResult {
        id,
        run_id,
        session_id,
        skill,
        provider,
        started_at,
        evidence: event.into_iter().collect(),
    })
}

fn tool_rows(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<ToolResult>, Box<dyn Error>> {
    let mut statement = connection.prepare("SELECT tool_name, COUNT(*), SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), COUNT(DISTINCT run_id) FROM tool_calls GROUP BY tool_name ORDER BY tool_name LIMIT ?1 OFFSET ?2")?;
    let rows = statement
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(ToolResult {
                name: row.get(0)?,
                calls: row.get(1)?,
                failed_calls: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                runs: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn tool_detail(
    connection: &Connection,
    name: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<ToolCallResult>, Box<dyn Error>> {
    let mut statement = connection
        .prepare("SELECT id FROM tool_calls WHERE tool_name = ?1 ORDER BY id LIMIT ?2 OFFSET ?3")?;
    let ids = statement
        .query_map(params![name, limit as i64, offset as i64], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| tool_call(connection, id))
        .collect()
}

fn skill_matrix(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<SkillCell>, Box<dyn Error>> {
    let mut statement = connection.prepare("SELECT run_id, skill_name, COUNT(*) FROM skill_calls WHERE run_id IS NOT NULL GROUP BY run_id, skill_name ORDER BY run_id, skill_name LIMIT ?1 OFFSET ?2")?;
    let rows = statement
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(SkillCell {
                run_id: row.get(0)?,
                skill: row.get(1)?,
                calls: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn filtered_skill_calls(
    connection: &Connection,
    run_id: Option<i64>,
    skill: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<(Vec<SkillCallResult>, i64), Box<dyn Error>> {
    let total = connection.query_row(
        "SELECT COUNT(*) FROM skill_calls WHERE (?1 IS NULL OR run_id = ?1) AND (?2 IS NULL OR skill_name = ?2)",
        params![run_id, skill],
        |row| row.get(0),
    )?;
    let ids = {
        let mut statement = connection.prepare(
            "SELECT id FROM skill_calls WHERE (?1 IS NULL OR run_id = ?1) AND (?2 IS NULL OR skill_name = ?2) ORDER BY id LIMIT ?3 OFFSET ?4",
        )?;
        let rows = statement
            .query_map(params![run_id, skill, limit as i64, offset as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    Ok((
        ids.into_iter()
            .map(|id| skill_call(connection, id))
            .collect::<Result<Vec<_>, _>>()?,
        total,
    ))
}

fn agent_skill_matrix(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<AgentSkillCell>, Box<dyn Error>> {
    let mut statement = connection.prepare(
        "SELECT r.agent, r.model, s.skill_name, COUNT(*) FROM runs r JOIN skill_calls s ON s.run_id = r.id WHERE r.agent IS NOT NULL OR r.model IS NOT NULL GROUP BY r.agent, r.model, s.skill_name ORDER BY r.agent, r.model, s.skill_name LIMIT ?1 OFFSET ?2",
    )?;
    let rows = statement
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(AgentSkillCell {
                agent: row.get(0)?,
                model: row.get(1)?,
                skill: row.get(2)?,
                calls: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn agent_models(
    connection: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<AgentModelResult>, Box<dyn Error>> {
    let mut statement = connection.prepare("SELECT agent, model, COUNT(*) FROM runs WHERE agent IS NOT NULL OR model IS NOT NULL GROUP BY agent, model ORDER BY agent, model LIMIT ?1 OFFSET ?2")?;
    let rows = statement
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(AgentModelResult {
                agent: row.get(0)?,
                model: row.get(1)?,
                runs: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn skill_rows(
    connection: &Connection,
    inactivity_days: u64,
) -> Result<Vec<SkillResult>, Box<dyn Error>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let cutoff = now
        .saturating_sub(u128::from(inactivity_days).saturating_mul(86_400_000))
        .min(i64::MAX as u128) as i64;
    let now = now.min(i64::MAX as u128) as i64;
    let mut map = std::collections::BTreeMap::<String, SkillResult>::new();
    {
        let mut statement = connection.prepare(
            "SELECT skill_name, source FROM skill_inventory ORDER BY skill_name, source",
        )?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (name, provider) = row?;
            let entry = map.entry(name.clone()).or_insert(SkillResult {
                name,
                installed: true,
                providers: Vec::new(),
                calls: 0,
                runs: 0,
                last_used: None,
                inactive: true,
                status: SkillStatus::NeverUsed,
            });
            if !entry.providers.contains(&provider) {
                entry.providers.push(provider);
            }
        }
    }
    {
        let mut statement = connection.prepare("SELECT skill_name, source FROM skill_calls GROUP BY skill_name, source ORDER BY skill_name, source")?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (name, provider) = row?;
            let entry = map.entry(name.clone()).or_insert(SkillResult {
                name,
                installed: false,
                providers: Vec::new(),
                calls: 0,
                runs: 0,
                last_used: None,
                inactive: true,
                status: SkillStatus::Unknown,
            });
            if !entry.providers.contains(&provider) {
                entry.providers.push(provider);
            }
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT skill_name, COUNT(*), COUNT(DISTINCT run_id),
                    MAX(CASE WHEN started_at <> '' AND started_at NOT GLOB '*[^0-9]*' THEN CAST(started_at AS INTEGER) END),
                    SUM(CASE WHEN started_at <> '' AND started_at NOT GLOB '*[^0-9]*' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN started_at <> '' AND started_at NOT GLOB '*[^0-9]*'
                                 AND CAST(started_at AS INTEGER) >= ?1
                                 AND CAST(started_at AS INTEGER) <= ?2
                             THEN 1 ELSE 0 END),
                    SUM(CASE WHEN started_at <> '' AND started_at NOT GLOB '*[^0-9]*'
                                 AND CAST(started_at AS INTEGER) <= ?2
                             THEN 1 ELSE 0 END)
             FROM skill_calls GROUP BY skill_name ORDER BY skill_name",
        )?;
        for row in statement.query_map(params![cutoff, now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })? {
            let (name, calls, runs, last_used, valid, recent, not_future) = row?;
            let entry = map.get_mut(&name).expect("skill call creates a map entry");
            entry.calls = calls;
            entry.runs = runs;
            entry.last_used = last_used.map(|timestamp| timestamp.to_string());
            entry.status = if recent > 0 {
                SkillStatus::Active
            } else if valid == calls && not_future == calls {
                SkillStatus::Dormant
            } else {
                SkillStatus::Unknown
            };
        }
    }
    for entry in map.values_mut() {
        if entry.calls == 0 {
            entry.status = SkillStatus::NeverUsed;
        }
        entry.inactive = matches!(entry.status, SkillStatus::NeverUsed | SkillStatus::Dormant);
    }
    Ok(map.into_values().collect())
}

fn evidence_detail(
    connection: &Connection,
    target: EvidenceTarget,
) -> Result<EvidenceDetail, Box<dyn Error>> {
    let (record, ids) = match target {
        EvidenceTarget::Run(id) => {
            let run = run_row(connection, id)?;
            (EvidenceRecord::Run(run.clone()), run.evidence)
        }
        EvidenceTarget::ToolCall(id) => {
            let call = tool_call(connection, id)?;
            (EvidenceRecord::ToolCall(call.clone()), call.evidence)
        }
        EvidenceTarget::SkillCall(id) => {
            let call = skill_call(connection, id)?;
            (EvidenceRecord::SkillCall(call.clone()), call.evidence)
        }
        EvidenceTarget::Inventory(id) => {
            let entry = connection.query_row(
                "SELECT id, skill_name, source, path FROM skill_inventory WHERE id = ?1",
                [id],
                |row| {
                    Ok(InventoryResult {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        provider: row.get(2)?,
                        path: row.get(3)?,
                    })
                },
            )?;
            (EvidenceRecord::Inventory(entry), Vec::new())
        }
    };
    let source_events = ids
        .into_iter()
        .map(|id| raw_event(connection, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EvidenceDetail {
        record,
        source_events,
    })
}

fn raw_event(connection: &Connection, id: i64) -> Result<RawEventResult, Box<dyn Error>> {
    Ok(connection.query_row(
        "SELECT e.id, s.source_type, s.source_path, e.source_identity, e.source_timestamp,
                e.observed_at, e.payload_hash, length(CAST(e.payload AS BLOB)),
                substr(e.payload, 1, ?2)
         FROM raw_events e JOIN sources s ON s.id = e.source_id WHERE e.id = ?1",
        params![id, MAX_RAW_EVENT_PREVIEW_CHARS],
        |row| {
            let payload_bytes: i64 = row.get(7)?;
            let payload: String = row.get(8)?;
            Ok(RawEventResult {
                id: row.get(0)?,
                source_type: row.get(1)?,
                source_path: row.get(2)?,
                source_identity: row.get(3)?,
                source_timestamp: row.get(4)?,
                observed_at: row.get(5)?,
                payload_hash: row.get(6)?,
                payload_truncated: payload_bytes > payload.len() as i64,
                payload_bytes,
                payload,
            })
        },
    )?)
}

fn summary(
    connection: &Connection,
    inactivity_days: u64,
    thresholds: WasteThresholds,
) -> Result<ReportResult, Box<dyn Error>> {
    let skills = skill_rows(connection, inactivity_days)?;
    let computed_findings = crate::analyzers::analyze(connection, thresholds)?.len();
    let mut durations = DurationHistogram::default();
    {
        let mut statement = connection.prepare("SELECT started_at, ended_at FROM runs")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let started_at: Option<String> = row.get(0)?;
            let ended_at: Option<String> = row.get(1)?;
            match duration_ms(started_at.as_deref(), ended_at.as_deref()) {
                Some(value) if value < 60_000 => durations.under_one_minute += 1,
                Some(value) if value < 300_000 => durations.one_to_five_minutes += 1,
                Some(value) if value < 900_000 => durations.five_to_fifteen_minutes += 1,
                Some(value) if value < 3_600_000 => durations.fifteen_to_sixty_minutes += 1,
                Some(_) => durations.over_sixty_minutes += 1,
                None => durations.unknown += 1,
            }
        }
    }
    let mut skill_status = SkillStatusCounts::default();
    for skill in skills {
        match skill.status {
            SkillStatus::Active => skill_status.active += 1,
            SkillStatus::Dormant => skill_status.dormant += 1,
            SkillStatus::NeverUsed => skill_status.never_used += 1,
            SkillStatus::Unknown => skill_status.unknown += 1,
        }
    }
    Ok(ReportResult {
        runs: count(connection, "runs")?,
        tasks: count(connection, "tasks")?,
        skills: count(connection, "skill_inventory")?,
        tool_calls: count(connection, "tool_calls")?,
        skill_calls: count(connection, "skill_calls")?,
        findings: count(connection, "findings")?,
        computed_findings,
        durations,
        skill_status,
    })
}

fn duration_ms(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let start = start?.parse::<i64>().ok()?;
    let end = end?.parse::<i64>().ok()?;
    end.checked_sub(start).filter(|duration| *duration >= 0)
}

fn count(connection: &Connection, table: &str) -> Result<i64, rusqlite::Error> {
    let sql = match table {
        "runs" => "SELECT COUNT(*) FROM runs",
        "tasks" => "SELECT COUNT(*) FROM tasks",
        "skill_inventory" => "SELECT COUNT(DISTINCT skill_name) FROM skill_inventory",
        "findings" => "SELECT COUNT(*) FROM findings",
        "tool_calls" => "SELECT COUNT(*) FROM tool_calls",
        "skill_calls" => "SELECT COUNT(*) FROM skill_calls",
        _ => unreachable!(),
    };
    connection.query_row(sql, [], |row| row.get(0))
}

pub fn write(mut writer: impl Write, output: &CommandOutput, json: bool) -> io::Result<()> {
    if json {
        serde_json::to_writer(&mut writer, output)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        writeln!(writer)
    } else {
        writeln!(writer, "{}", text(output))
    }
}

fn text(output: &CommandOutput) -> String {
    match &output.data {
        CommandData::Waste(result) if result.items.is_empty() => "No waste findings match the stored evidence and configured thresholds.".to_owned(),
        CommandData::Waste(result) => result.items.iter().map(finding_text).collect::<Vec<_>>().join("\n"),
        CommandData::Skills(result) if result.items.is_empty() => "No skill data is available.".to_owned(),
        CommandData::Skills(result) => result.items.iter().map(|skill| format!("{}: {} calls across {} runs{} ({:?})", skill.name, skill.calls, skill.runs, if skill.installed { " (installed)" } else { " (not installed)" }, skill.status)).collect::<Vec<_>>().join("\n"),
        CommandData::Scan(result) => format!("Scanned {} sources and imported {} events.", result.sources_scanned, result.events_imported),
        CommandData::List(result) if result.items.is_empty() => "No run data is available.".to_owned(),
        CommandData::List(result) => result.items.iter().map(|run| format!("run={} task={:?} outcome={:?}", run.id, run.task_id, run.outcome)).collect::<Vec<_>>().join("\n"),
        CommandData::Report(result) => format!("Stored data: {} runs, {} tasks, {} skills, {} tool calls, {} skill calls, {} stored findings; {} computed findings.", result.runs, result.tasks, result.skills, result.tool_calls, result.skill_calls, result.findings, result.computed_findings),
    }
}

fn finding_text(finding: &Finding) -> String {
    let baseline = finding
        .threshold
        .as_ref()
        .and_then(|threshold| threshold.baseline.as_ref())
        .map_or_else(String::new, |baseline| format!(" (baseline: {baseline})"));
    let evidence = finding
        .evidence
        .iter()
        .map(|item| {
            let timestamp = item
                .observed_at
                .as_ref()
                .map_or_else(String::new, |timestamp| format!("@{timestamp}"));
            format!("{}#{}{}", item.entity_type, item.id, timestamp)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{:?}: {}{} [evidence: {}]",
        finding.finding_type, finding.explanation, baseline, evidence
    )
}

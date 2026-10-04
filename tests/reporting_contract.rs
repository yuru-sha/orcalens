use orcalens::{
    cli::Command,
    reporting::{
        self, CommandData, CommandOutput, EvidenceTarget, ReportData, ReportRequest, ReportView,
        SkillStatus,
    },
};
use rusqlite::Connection;

fn database() -> Connection {
    orcalens::storage::open(":memory:").expect("open migrated fixture database")
}

#[test]
fn json_output_serializes_populated_run_records_in_the_shared_envelope() {
    let output = CommandOutput {
        command: Command::Runs,
        data: CommandData::List(reporting::ListResult {
            items: vec![reporting::RunResult {
                id: 12,
                task_id: Some(5),
                task_title: Some("task title".to_owned()),
                started_at: Some("1000".to_owned()),
                ended_at: Some("2000".to_owned()),
                duration_ms: Some(1000),
                outcome: Some("completed".to_owned()),
                agent: None,
                model: None,
                sessions: vec![8],
                tool_calls: 1,
                skill_calls: 0,
                evidence: vec![21],
            }],
        }),
    };
    let mut bytes = Vec::new();
    reporting::write(&mut bytes, &output, true).expect("write JSON");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("parse JSON");
    assert_eq!(value["command"], "runs");
    assert_eq!(value["data"]["items"][0]["id"], 12);
    assert_eq!(value["data"]["items"][0]["task_id"], 5);
    assert_eq!(
        value["data"]["items"][0]["evidence"],
        serde_json::json!([21])
    );
}

#[test]
fn task_run_evidence_and_tool_views_follow_recorded_links() {
    let connection = database();
    connection.execute("INSERT INTO tasks(id, source, source_key, title) VALUES (7, 'orca', 'task-a', 'Task A')", []).unwrap();
    connection.execute("INSERT INTO sources(id, source_type, source_path, source_identity) VALUES (1, 'orca', 'journal', 'source')", []).unwrap();
    connection.execute_batch(
        "INSERT INTO raw_events(id, source_id, source_identity, observed_at, source_timestamp, payload, payload_hash) VALUES
            (11, 1, 'event-a', 'observed', '1000', '{\"kind\":\"turn\"}', 'hash-a'),
            (12, 1, 'event-b', 'observed', '1100', '{\"kind\":\"tool\"}', 'hash-b'),
            (13, 1, 'event-c', 'observed', '1200', '{\"kind\":\"skill\"}', 'hash-c');
         INSERT INTO runs(id, task_id, started_at, ended_at, outcome, start_event_id, end_event_id) VALUES (3, 7, '1000', '3000', 'completed', 11, 11);
         INSERT INTO sessions(id, run_id, provider, provider_session_id) VALUES (9, 3, 'orca_journal', 'session-9');
         INSERT INTO run_sessions(run_id, session_id, source_event_id) VALUES (3, 9, 11);
         INSERT INTO tool_calls(id, run_id, session_id, tool_name, status, start_event_id, end_event_id) VALUES (4, 3, 9, 'read', 'failed', 12, 12);
         INSERT INTO skill_calls(id, run_id, session_id, skill_name, source, started_at, source_event_id, dedup_key) VALUES (5, 3, 9, 'review', 'omp', '1500', 13, 'skill-5');",
    ).unwrap();

    let runs = reporting::query(&connection, &ReportRequest::new(ReportView::Runs)).unwrap();
    let ReportData::Runs(runs) = runs.view else {
        panic!("runs view")
    };
    assert_eq!(runs.items.len(), 1);
    assert_eq!(runs.items[0].task_title.as_deref(), Some("Task A"));
    assert_eq!(runs.items[0].duration_ms, Some(2000));
    assert_eq!(runs.items[0].sessions, vec![9]);
    assert_eq!(runs.items[0].tool_calls, 1);
    assert_eq!(runs.items[0].skill_calls, 1);
    assert_eq!(runs.items[0].evidence, vec![11, 12, 13]);

    let task = reporting::query(&connection, &ReportRequest::new(ReportView::Task(7))).unwrap();
    let ReportData::Task(task) = task.view else {
        panic!("task view")
    };
    assert_eq!(task.task.runs, 1);
    assert_eq!(task.runs[0].id, 3);

    let run = reporting::query(&connection, &ReportRequest::new(ReportView::Run(3))).unwrap();
    let ReportData::Run(run) = run.view else {
        panic!("run view")
    };
    assert_eq!(run.tool_calls[0].id, 4);
    assert_eq!(run.skill_calls[0].id, 5);

    let evidence = reporting::query(
        &connection,
        &ReportRequest::new(ReportView::Evidence(EvidenceTarget::ToolCall(4))),
    )
    .unwrap();
    let ReportData::Evidence(evidence) = evidence.view else {
        panic!("evidence view")
    };
    assert_eq!(
        evidence
            .source_events
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        vec![12]
    );
    assert_eq!(evidence.source_events[0].payload_bytes, 15);
    let reference = serde_json::to_value(&evidence.source_events[0]).unwrap();
    assert!(reference.get("payload").is_none());

    let mut evidence_request = ReportRequest::new(ReportView::Evidence(EvidenceTarget::Run(3)));
    evidence_request.limit = 1;
    evidence_request.offset = 1;
    let evidence_page = reporting::query(&connection, &evidence_request).unwrap();
    assert_eq!(evidence_page.metadata.total, 3);
    assert_eq!(evidence_page.metadata.offset, 1);
    let ReportData::Evidence(evidence_page) = evidence_page.view else {
        panic!("evidence view")
    };
    assert_eq!(
        evidence_page
            .source_events
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        vec![12]
    );

    let raw_event = reporting::query(
        &connection,
        &ReportRequest::new(ReportView::SourceEvent(12)),
    )
    .unwrap();
    let ReportData::SourceEvent(raw_event) = raw_event.view else {
        panic!("source event view")
    };
    assert_eq!(raw_event.payload, "{\"kind\":\"tool\"}");
    assert_eq!(raw_event.payload_bytes, 15);
    assert!(!raw_event.payload_truncated);

    let tools = reporting::query(&connection, &ReportRequest::new(ReportView::Tools)).unwrap();
    let ReportData::Tools(tools) = tools.view else {
        panic!("tools view")
    };
    assert_eq!(
        tools,
        vec![reporting::ToolResult {
            name: "read".to_owned(),
            calls: 1,
            failed_calls: 1,
            runs: 1
        }]
    );

    let matrix =
        reporting::query(&connection, &ReportRequest::new(ReportView::SkillMatrix)).unwrap();
    let ReportData::SkillMatrix(matrix) = matrix.view else {
        panic!("skill matrix view")
    };
    assert_eq!(
        matrix,
        vec![reporting::SkillCell {
            run_id: 3,
            skill: "review".to_owned(),
            calls: 1
        }]
    );
    let drilldown = reporting::query(
        &connection,
        &ReportRequest::new(ReportView::SkillCalls {
            run_id: Some(3),
            skill: Some("review".to_owned()),
        }),
    )
    .unwrap();
    let ReportData::SkillCalls(calls) = drilldown.view else {
        panic!("skill calls view")
    };
    assert_eq!(drilldown.metadata.total, matrix[0].calls);
    assert_eq!(calls[0].id, 5);
}

#[test]
fn skills_keep_all_providers_and_classify_partial_timestamps_honestly() {
    let connection = database();
    connection
        .execute_batch(
            "INSERT INTO runs(id) VALUES (1), (2), (3), (4), (5), (6);
         INSERT INTO skill_inventory(skill_name, source, path) VALUES
            ('unused', 'claude', '/claude/unused'), ('unused', 'codex', '/codex/unused');
         INSERT INTO skill_calls(id, run_id, skill_name, source, started_at, dedup_key) VALUES
            (1, 1, 'active', 'omp', CAST(CAST(strftime('%s', 'now') AS INTEGER) * 1000 AS TEXT), 'active-recent'),
            (2, 2, 'active', 'claude', NULL, 'active-missing'),
            (3, 3, 'dormant', 'codex', '1', 'dormant-old'),
            (4, 4, 'unknown', 'omp', NULL, 'unknown-missing'),
            (5, 5, 'unknown', 'claude', 'invalid', 'unknown-invalid'),
            (6, 6, 'future', 'fixture', '9999999999999', 'future');",
        )
        .unwrap();

    let request = ReportRequest::new(ReportView::Skills);
    let snapshot = reporting::query(&connection, &request).unwrap();
    let ReportData::Skills(result) = snapshot.view else {
        panic!("skills view")
    };
    let status = |name: &str| {
        result
            .items
            .iter()
            .find(|skill| skill.name == name)
            .unwrap()
    };
    assert_eq!(status("active").status, SkillStatus::Active);
    assert_eq!(status("active").calls, 2);
    assert_eq!(status("active").runs, 2);
    assert_eq!(status("active").providers, vec!["claude", "omp"]);
    assert!(!status("active").inactive);
    assert_eq!(status("dormant").status, SkillStatus::Dormant);
    assert!(status("dormant").inactive);
    assert_eq!(status("unused").status, SkillStatus::NeverUsed);
    assert_eq!(status("unknown").status, SkillStatus::Unknown);
    assert_eq!(status("future").status, SkillStatus::Unknown);
    assert!(!status("unknown").inactive);
    assert!(!status("future").inactive);
    assert_eq!(status("unused").providers, vec!["claude", "codex"]);
}

#[test]
fn summaries_distinguish_stored_and_computed_findings_and_attribute_only_known_runs() {
    let connection = database();
    connection
        .execute_batch(
            "INSERT INTO tasks(id, source, source_key) VALUES (1, 'orca', 'task-1');
         INSERT INTO runs(id, task_id, started_at, ended_at, agent, model) VALUES
            (1, 1, '1000', '2000', 'agent-a', 'model-a'),
            (2, NULL, NULL, NULL, NULL, NULL);
         INSERT INTO findings(id, run_id, finding_type, severity, observed_at, details) VALUES
            (1, 1, 'stored', 'low', '1000', '{}');",
        )
        .unwrap();
    let snapshot = reporting::query(&connection, &ReportRequest::new(ReportView::Summary)).unwrap();
    let ReportData::Summary(summary) = snapshot.view else {
        panic!("summary view")
    };
    assert_eq!(summary.runs, 2);
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.findings, 1);
    assert_eq!(summary.computed_findings, 0);
    assert_eq!(summary.durations.under_one_minute, 1);
    assert_eq!(summary.durations.unknown, 1);

    let attribution =
        reporting::query(&connection, &ReportRequest::new(ReportView::AgentModels)).unwrap();
    let ReportData::AgentModels(attribution) = attribution.view else {
        panic!("agent model view")
    };
    assert_eq!(
        attribution,
        vec![reporting::AgentModelResult {
            agent: Some("agent-a".to_owned()),
            model: Some("model-a".to_owned()),
            runs: 1
        }]
    );
}

#[test]
fn report_reader_opens_existing_database_without_creating_or_migrating_it() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("analytics.db");
    drop(orcalens::storage::open(&path).expect("create database"));
    let reader =
        reporting::ReportReader::open_read_only(&path).expect("open read-only report reader");
    let snapshot = reader
        .query(&ReportRequest::new(ReportView::Runs))
        .expect("query read-only report");
    let ReportData::Runs(runs) = snapshot.view else {
        panic!("runs view")
    };
    assert!(runs.items.is_empty());
}

#[test]
fn run_pages_keep_stable_id_order_and_report_the_unpaged_total() {
    let connection = database();
    connection
        .execute_batch("INSERT INTO runs(id) VALUES (9), (2), (5);")
        .unwrap();

    let mut request = ReportRequest::new(ReportView::Runs);
    request.limit = 1;
    request.offset = 1;
    let snapshot = reporting::query(&connection, &request).unwrap();
    let ReportData::Runs(runs) = snapshot.view else {
        panic!("runs view")
    };
    assert_eq!(snapshot.metadata.total, 3);
    assert_eq!(snapshot.metadata.limit, 1);
    assert_eq!(snapshot.metadata.offset, 1);
    assert_eq!(
        runs.items.iter().map(|run| run.id).collect::<Vec<_>>(),
        vec![5]
    );
}

#[test]
fn raw_source_event_payload_preview_is_bounded_and_marks_truncation() {
    let connection = database();
    connection
        .execute(
            "INSERT INTO sources(id, source_type, source_path, source_identity) VALUES (1, 'fixture', 'local', 'large-source')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO raw_events(id, source_id, source_identity, observed_at, payload, payload_hash)
             VALUES (1, 1, 'large-event', 'observed', replace(printf('%300001s', 'x'), ' ', 'x'), 'hash')",
            [],
        )
        .unwrap();

    let snapshot =
        reporting::query(&connection, &ReportRequest::new(ReportView::SourceEvent(1))).unwrap();
    let ReportData::SourceEvent(event) = snapshot.view else {
        panic!("source event view")
    };
    assert_eq!(event.payload_bytes, 300_001);
    assert_eq!(event.payload.len(), 250_000);
    assert!(event.payload_truncated);
}

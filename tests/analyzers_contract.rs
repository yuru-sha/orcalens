use orcalens::analyzers::{analyze, FindingKind, WasteThresholds};
use rusqlite::Connection;

fn fixture() -> Connection {
    let connection = Connection::open_in_memory().expect("open fixture database");
    connection
        .execute_batch(
            "CREATE TABLE skill_inventory (id INTEGER PRIMARY KEY, skill_name TEXT NOT NULL, source TEXT NOT NULL, path TEXT NOT NULL);
             CREATE TABLE skill_calls (id INTEGER PRIMARY KEY, run_id INTEGER, session_id INTEGER, skill_name TEXT NOT NULL, started_at TEXT, ended_at TEXT, status TEXT, arguments_hash TEXT);
             CREATE TABLE tool_calls (id INTEGER PRIMARY KEY, run_id INTEGER, session_id INTEGER, tool_name TEXT NOT NULL, started_at TEXT, ended_at TEXT, status TEXT, input_hash TEXT);
             CREATE TABLE runs (id INTEGER PRIMARY KEY, task_id INTEGER, started_at TEXT, ended_at TEXT, outcome TEXT);",
        )
        .expect("create fixture schema");
    connection
}

fn thresholds() -> WasteThresholds {
    WasteThresholds {
        inactivity_days: 2,
        repeat_count: 2,
        long_run_multiplier: 2,
    }
}

#[test]
fn reports_supported_signals_with_complete_evidence_and_stable_order() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO skill_inventory VALUES (1, 'unused', 'test', '/unused');
             INSERT INTO skill_inventory VALUES (2, 'old', 'test', '/old');
             INSERT INTO skill_inventory VALUES (3, 'unused', 'other', '/other/unused');
             INSERT INTO skill_inventory VALUES (4, 'old', 'other', '/other/old');
             INSERT INTO skill_calls VALUES (1, 10, 20, 'old', '1000', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (2, 11, 21, 'activity', '172801000', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (3, 10, 20, 'repeat', '172800000', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (4, 10, 20, 'repeat', '172800001', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (5, 10, 20, 'repeat', '172800002', NULL, NULL, NULL);
             INSERT INTO tool_calls VALUES (1, 10, 20, 'read', '172800010', NULL, 'failed', 'abc');
             INSERT INTO tool_calls VALUES (2, 10, 20, 'read', '172800020', NULL, 'succeeded', 'abc');
             INSERT INTO tool_calls VALUES (3, 10, 20, 'edit', NULL, NULL, 'failed', NULL);
             INSERT INTO tool_calls VALUES (4, 10, 20, 'read', '172800030', NULL, 'succeeded', 'abc');
             INSERT INTO runs VALUES (10, NULL, '1000', '4000000', NULL);
             INSERT INTO runs VALUES (11, NULL, '1000', '2000', 'interrupted');
             INSERT INTO runs VALUES (12, NULL, '1000', NULL, NULL);
             INSERT INTO runs VALUES (20, 7, '1', '60001', NULL);
             INSERT INTO runs VALUES (21, 7, '60002', '120002', NULL);
             INSERT INTO runs VALUES (22, 7, '120003', '180003', NULL);
             INSERT INTO runs VALUES (23, 7, '180004', '300004', NULL);",
        )
        .expect("populate fixture");

    let findings = analyze(&connection, thresholds()).expect("analyze fixture");
    let kinds: Vec<_> = findings
        .iter()
        .map(|finding| finding.finding_type.clone())
        .collect();
    assert!(kinds.contains(&FindingKind::NeverInvokedSkill));
    assert!(kinds.contains(&FindingKind::InactiveSkill));
    assert!(kinds.contains(&FindingKind::RepeatedSkillCall));
    assert!(kinds.contains(&FindingKind::RepeatedIdenticalToolCall));
    assert!(kinds.contains(&FindingKind::FailedToolCall));
    assert!(kinds.contains(&FindingKind::LongRun));
    assert!(kinds.contains(&FindingKind::InterruptedRun));
    let never_invoked: Vec<_> = findings
        .iter()
        .filter(|finding| finding.finding_type == FindingKind::NeverInvokedSkill)
        .collect();
    assert_eq!(never_invoked.len(), 1);
    assert_eq!(
        never_invoked[0]
            .evidence
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    let inactive = findings
        .iter()
        .find(|finding| {
            finding.finding_type == FindingKind::InactiveSkill
                && finding.affected.skill.as_deref() == Some("old")
        })
        .expect("inactive skill finding");
    assert!(inactive.evidence.iter().any(|item| {
        item.entity_type == "skill_call"
            && item.id == 2
            && item.observed_at.as_deref() == Some("172801000")
    }));
    assert_eq!(
        inactive
            .evidence
            .iter()
            .filter(|item| item.entity_type == "skill_inventory")
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [2, 4]
    );
    assert!(!findings
        .iter()
        .any(|finding| finding.affected.tool.as_deref() == Some("edit")
            && finding.finding_type == FindingKind::RepeatedIdenticalToolCall));
    assert!(!findings
        .iter()
        .any(|finding| finding.affected.run_id == Some(12)
            && finding.finding_type == FindingKind::LongRun));
    let repeated_skill = findings
        .iter()
        .find(|finding| finding.finding_type == FindingKind::RepeatedSkillCall)
        .expect("repeated skill finding");
    assert_eq!(
        repeated_skill
            .evidence
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );
    let repeated_tool = findings
        .iter()
        .find(|finding| finding.finding_type == FindingKind::RepeatedIdenticalToolCall)
        .expect("repeated tool finding");
    assert_eq!(
        repeated_tool
            .evidence
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [1, 2, 4]
    );
    let long_run = findings
        .iter()
        .find(|finding| finding.finding_type == FindingKind::LongRun)
        .expect("long run finding");
    assert_eq!(long_run.affected.run_id, Some(23));
    assert_eq!(long_run.evidence.len(), 4);
    assert_eq!(long_run.metric.value, 120_000);
    assert!(findings.iter().all(|finding| !finding.evidence.is_empty()));
    assert!(findings.iter().all(|finding| finding.metric.value >= 0));

    let first = serde_json::to_vec(&findings).expect("serialize findings");
    let second = serde_json::to_vec(&analyze(&connection, thresholds()).expect("repeat analysis"))
        .expect("serialize findings");
    assert_eq!(first, second);
}

#[test]
fn relative_long_run_uses_past_task_median_and_requires_three_observations() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO skill_calls VALUES (1, 1, 1, 'once', '1', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (2, 1, 1, 'twice', '2', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (3, 1, 1, 'twice', '3', NULL, NULL, NULL);
             INSERT INTO runs VALUES (1, 1, '1', '60001', NULL);
             INSERT INTO runs VALUES (2, 1, '60002', '120002', NULL);
             INSERT INTO runs VALUES (3, 1, '120003', '180003', NULL);
             INSERT INTO runs VALUES (4, 1, '180004', '300003', NULL);
             INSERT INTO runs VALUES (10, 2, '1', '60001', NULL);
             INSERT INTO runs VALUES (11, 2, '60002', '120002', NULL);
             INSERT INTO runs VALUES (12, 2, '120003', '180003', NULL);
             INSERT INTO runs VALUES (13, 2, '180004', '300004', NULL);
             INSERT INTO runs VALUES (20, 3, '1', '60001', NULL);
             INSERT INTO runs VALUES (21, 3, '60002', '120002', NULL);
             INSERT INTO runs VALUES (22, 3, '120003', '240002', NULL);
             INSERT INTO runs VALUES (40, 4, '1', '60001', NULL);
             INSERT INTO runs VALUES (41, 4, '60002', '120002', NULL);
             INSERT INTO runs VALUES (42, 4, '120003', '180003', NULL);
             INSERT INTO runs VALUES (43, 4, '180004', '680004', NULL);
             INSERT INTO runs VALUES (50, 5, '1', '200001', NULL);
             INSERT INTO runs VALUES (51, 5, '2', '200002', NULL);
             INSERT INTO runs VALUES (52, 5, '3', '200003', NULL);
             INSERT INTO runs VALUES (53, 5, '100000', '500000', NULL);",
        )
        .expect("populate relative threshold fixture");
    let findings = analyze(&connection, thresholds()).expect("analyze relative baseline");
    assert!(findings.iter().any(
        |finding| finding.finding_type == FindingKind::RepeatedSkillCall
            && finding.affected.skill.as_deref() == Some("twice")
    ));
    assert!(!findings.iter().any(
        |finding| finding.finding_type == FindingKind::RepeatedSkillCall
            && finding.affected.skill.as_deref() == Some("once")
    ));
    for run_id in [4, 22, 42, 50, 52, 53] {
        assert!(!findings
            .iter()
            .any(|finding| finding.finding_type == FindingKind::LongRun
                && finding.affected.run_id == Some(run_id)));
    }
    let boundary_finding = findings
        .iter()
        .find(|finding| {
            finding.finding_type == FindingKind::LongRun && finding.affected.run_id == Some(13)
        })
        .expect("run at relative threshold boundary");
    assert_eq!(boundary_finding.metric.value, 120_000);
    assert_eq!(boundary_finding.evidence.len(), 4);
    assert_eq!(boundary_finding.threshold.as_ref().unwrap().value, 2);
    let later_run = findings
        .iter()
        .find(|finding| {
            finding.finding_type == FindingKind::LongRun && finding.affected.run_id == Some(43)
        })
        .expect("later run with sufficient historical baseline");
    assert_eq!(later_run.evidence.len(), 4);
    assert!(later_run.evidence.iter().any(|evidence| evidence.id == 42));
}

#[test]
fn skill_recency_uses_numeric_timestamp_order() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO skill_inventory VALUES (1, 'recent', 'test', '/recent');
             INSERT INTO skill_calls VALUES (1, NULL, NULL, 'recent', '9', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (2, NULL, NULL, 'recent', '86400009', NULL, NULL, NULL);",
        )
        .expect("populate numeric timestamp fixture");

    let findings = analyze(
        &connection,
        WasteThresholds {
            inactivity_days: 1,
            repeat_count: 2,
            long_run_multiplier: 2,
        },
    )
    .expect("analyze numeric timestamps");
    assert!(!findings
        .iter()
        .any(|finding| finding.finding_type == FindingKind::InactiveSkill));
}

#[test]
fn terminal_only_tool_evidence_uses_end_timestamp() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO tool_calls VALUES
                 (1, 10, 20, 'read', NULL, '2000', 'failed', 'failed-hash'),
                 (2, 10, 20, 'write', NULL, '3000', 'succeeded', 'same-hash'),
                 (3, 10, 20, 'write', NULL, '4000', 'succeeded', 'same-hash');",
        )
        .expect("populate terminal-only tool fixture");
    let findings = analyze(&connection, thresholds()).expect("analyze terminal-only tools");
    let failed = findings
        .iter()
        .find(|finding| finding.finding_type == FindingKind::FailedToolCall)
        .expect("failed tool finding");
    assert_eq!(failed.evidence[0].id, 1);
    assert_eq!(failed.evidence[0].observed_at.as_deref(), Some("2000"));
    let repeated = findings
        .iter()
        .find(|finding| finding.finding_type == FindingKind::RepeatedIdenticalToolCall)
        .expect("repeated terminal-only tool finding");
    assert_eq!(
        repeated
            .evidence
            .iter()
            .map(|item| (item.id, item.observed_at.as_deref()))
            .collect::<Vec<_>>(),
        [(2, Some("3000")), (3, Some("4000"))]
    );
}

#[test]
fn inactivity_finding_cites_latest_end_time_for_same_call() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO skill_inventory VALUES (1, 'ended', 'test', '/ended');
             INSERT INTO skill_calls VALUES (1, NULL, NULL, 'ended', '1000', '172801000', NULL, NULL);",
        )
        .expect("populate end timestamp fixture");
    let findings = analyze(
        &connection,
        WasteThresholds {
            inactivity_days: 2,
            repeat_count: 2,
            long_run_multiplier: 2,
        },
    )
    .expect("analyze end timestamp");
    let inactive = findings
        .iter()
        .find(|finding| finding.finding_type == FindingKind::InactiveSkill)
        .expect("inactive skill finding");
    assert!(inactive.evidence.iter().any(|item| {
        item.entity_type == "skill_call"
            && item.id == 1
            && item.observed_at.as_deref() == Some("1000")
    }));
    assert!(inactive.evidence.iter().any(|item| {
        item.entity_type == "skill_call"
            && item.id == 1
            && item.observed_at.as_deref() == Some("172801000")
    }));
    assert!(inactive
        .threshold
        .as_ref()
        .unwrap()
        .baseline
        .as_deref()
        .unwrap()
        .contains("skill_call 1 at 172801000"));
}

#[test]
fn undated_skill_calls_prevent_inactivity_findings() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO skill_inventory VALUES (1, 'old', 'test', '/old');
             INSERT INTO skill_calls VALUES (1, NULL, NULL, 'old', '1000', NULL, NULL, NULL);
             INSERT INTO skill_calls VALUES (2, NULL, NULL, 'old', NULL, NULL, NULL, NULL);
             INSERT INTO runs VALUES (1, NULL, '1', '172801000', NULL);",
        )
        .expect("populate mixed timestamp fixture");
    let findings = analyze(
        &connection,
        WasteThresholds {
            inactivity_days: 2,
            repeat_count: 2,
            long_run_multiplier: 2,
        },
    )
    .expect("analyze mixed timestamps");
    assert!(!findings
        .iter()
        .any(|finding| finding.finding_type == FindingKind::InactiveSkill));
}

#[test]
fn empty_and_incomplete_evidence_does_not_create_unsupported_findings() {
    let connection = fixture();
    connection
        .execute_batch(
            "INSERT INTO runs VALUES (1, NULL, NULL, NULL, NULL);
             INSERT INTO tool_calls VALUES (1, NULL, NULL, 'read', NULL, NULL, NULL, NULL);",
        )
        .expect("populate incomplete fixture");
    let findings = analyze(&connection, thresholds()).expect("analyze incomplete fixture");
    assert!(findings.is_empty());
}

use serde_json::Value;
use std::process::Command;

fn run(home: &std::path::Path, database: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orcalens"))
        .args(args)
        .env("HOME", home)
        .env("ORCALENS_DB", database)
        .output()
        .expect("run orcalens")
}

#[test]
fn waste_json_and_human_output_use_stored_fixture_evidence() {
    let home = tempfile::tempdir().expect("temporary home");
    let database = home.path().join("fixture.db");
    let initialized = run(home.path(), &database, &["runs", "--json"]);
    assert!(initialized.status.success(), "{:?}", initialized.stderr);
    let connection = rusqlite::Connection::open(&database).expect("open initialized database");
    connection.execute_batch(
        "INSERT INTO skill_inventory(skill_name, source, path) VALUES ('unused-skill', 'fixture', '/fixture/unused');
         INSERT INTO tasks(id, source, source_key) VALUES (1, 'fixture', 'task-1');
         INSERT INTO runs(id, started_at, ended_at, outcome) VALUES (42, '1000', '4000000', 'interrupted');
         INSERT INTO runs(id, task_id, started_at, ended_at) VALUES
             (50, 1, '1', '60001'), (51, 1, '60002', '120002'),
             (52, 1, '120003', '180003'), (53, 1, '180004', '300004');
         INSERT INTO sessions(id, run_id, provider, provider_session_id) VALUES (5, 42, 'fixture', 'session-5');
         INSERT INTO tool_calls(id, run_id, session_id, tool_name, started_at, ended_at, status, input_hash)
         VALUES (9, 42, 5, 'read', NULL, '2000', 'failed', 'input-hash'),
                (10, 42, 5, 'read', '3000', NULL, 'succeeded', 'alpha'),
                (11, 42, 5, 'read', '4000', NULL, 'succeeded', 'alpha'),
                (12, 42, 5, 'read', '5000', NULL, 'succeeded', 'beta'),
                (13, 42, 5, 'read', '6000', NULL, 'succeeded', 'beta');",
    ).expect("insert normalized source evidence");
    drop(connection);

    let output = run(home.path(), &database, &["waste", "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let value: Value = serde_json::from_slice(&output.stdout).expect("parse waste JSON");
    assert_eq!(value["command"], "waste");
    let items = value["data"]["items"].as_array().expect("items array");
    assert!(items
        .iter()
        .any(|item| item["finding_type"] == "never_invoked_skill"));
    assert!(items
        .iter()
        .any(|item| item["finding_type"] == "failed_tool_call"));
    let failed = items
        .iter()
        .find(|item| item["finding_type"] == "failed_tool_call")
        .expect("failed tool finding");
    assert!(failed["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|evidence| evidence["observed_at"] == "2000"));
    assert!(items.iter().any(|item| item["finding_type"] == "long_run"));
    let long_run = items
        .iter()
        .find(|item| item["finding_type"] == "long_run")
        .expect("long-run finding");
    assert_eq!(long_run["affected"]["run_id"], 53);
    assert_eq!(long_run["evidence"].as_array().unwrap().len(), 4);
    assert!(items
        .iter()
        .any(|item| item["finding_type"] == "interrupted_run"));
    for item in items {
        assert!(item["finding_type"].is_string());
        assert!(item["evidence"]
            .as_array()
            .is_some_and(|evidence| !evidence.is_empty()));
        assert!(item["affected"].is_object());
        assert!(item["metric"]["value"].is_number());
        assert!(item["threshold"].is_object());
        assert!(item["explanation"].is_string());
    }

    let output = run(home.path(), &database, &["waste"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let human = String::from_utf8(output.stdout).expect("human output is UTF-8");
    assert!(human.contains("Installed skill 'unused-skill'"));
    assert!(human.contains("tool_call#10@3000"));
    assert!(human.contains("tool_call#9@2000"));
    assert!(human.contains("tool_call#12@5000"));
    assert!(human.contains("baseline: tool name and normalized input hash alpha"));
    assert!(human.contains("status failed"));
}

#[test]
fn waste_human_output_explains_when_no_findings_match() {
    let home = tempfile::tempdir().expect("temporary home");
    let database = home.path().join("empty.db");
    let initialized = run(home.path(), &database, &["runs", "--json"]);
    assert!(initialized.status.success(), "{:?}", initialized.stderr);

    let output = run(home.path(), &database, &["waste"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("human output is UTF-8")
            .trim(),
        "No waste findings match the stored evidence and configured thresholds."
    );
}

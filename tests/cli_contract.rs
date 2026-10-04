use serde_json::Value;
use std::process::Command;

#[test]
fn help_lists_all_supported_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_orcalens"))
        .arg("--help")
        .output()
        .expect("run help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("help is UTF-8");
    let commands = help
        .split_once("Commands:")
        .map(|(_, commands)| commands)
        .expect("help has a command section");
    for command in ["scan", "runs", "skills", "waste", "report", "dashboard"] {
        assert!(
            commands
                .lines()
                .any(|line| line.split_whitespace().next() == Some(command)),
            "help should list {command}"
        );
    }
}

#[test]
fn dashboard_requires_an_existing_database_without_creating_one() {
    let home = tempfile::tempdir().expect("temporary home");
    let database = home.path().join("orcalens.db");
    let output = Command::new(env!("CARGO_BIN_EXE_orcalens"))
        .arg("dashboard")
        .env("HOME", home.path())
        .env("ORCALENS_DB", &database)
        .output()
        .expect("run dashboard");
    assert!(!output.status.success());
    assert!(!database.exists());
}

#[test]
fn each_command_emits_the_shared_json_envelope() {
    let home = tempfile::tempdir().expect("temporary home");
    let database = home.path().join("orcalens.db");
    for command in ["scan", "runs", "skills", "waste", "report"] {
        let output = Command::new(env!("CARGO_BIN_EXE_orcalens"))
            .arg(command)
            .arg("--json")
            .env("HOME", home.path())
            .env("ORCALENS_DB", &database)
            .output()
            .expect("run command");
        assert!(output.status.success(), "{command}: {:?}", output.stderr);
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON output");
        assert_eq!(value["command"], command);
        assert!(value["data"].is_object());
    }
}

#[test]
fn skills_lists_installed_skills_and_unused_filter() {
    let home = tempfile::tempdir().expect("temporary home");
    let skill = home.path().join(".agents/skills/review");
    std::fs::create_dir_all(&skill).expect("create skill directory");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: code-review\ndescription: review code\n---\n",
    )
    .expect("write skill");
    let database = home.path().join("orcalens.db");
    let output = Command::new(env!("CARGO_BIN_EXE_orcalens"))
        .args(["skills", "--unused", "--json"])
        .env("HOME", home.path())
        .env("ORCALENS_DB", &database)
        .output()
        .expect("run skills");
    assert!(output.status.success(), "{:?}", output.stderr);
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["command"], "skills");
    assert_eq!(value["data"]["items"][0]["name"], "code-review");
    assert_eq!(value["data"]["items"][0]["installed"], true);
    assert_eq!(value["data"]["items"][0]["calls"], 0);
    assert_eq!(value["data"]["items"][0]["inactive"], true);
}

#[test]
fn runs_cli_returns_populated_typed_items() {
    let home = tempfile::tempdir().expect("temporary home");
    let database = home.path().join("orcalens.db");
    let initialized = Command::new(env!("CARGO_BIN_EXE_orcalens"))
        .args(["runs", "--json"])
        .env("HOME", home.path())
        .env("ORCALENS_DB", &database)
        .output()
        .expect("initialize database");
    assert!(initialized.status.success(), "{:?}", initialized.stderr);

    let connection = rusqlite::Connection::open(&database).expect("open fixture database");
    connection
        .execute(
            "INSERT INTO tasks(id, source, source_key, title) VALUES (7, 'fixture', 'task-7', 'Populated task')",
            [],
        )
        .expect("insert task");
    connection
        .execute(
            "INSERT INTO runs(id, task_id, started_at, ended_at, outcome) VALUES (12, 7, '1000', '3000', 'completed')",
            [],
        )
        .expect("insert run");
    drop(connection);

    let output = Command::new(env!("CARGO_BIN_EXE_orcalens"))
        .args(["runs", "--json"])
        .env("HOME", home.path())
        .env("ORCALENS_DB", &database)
        .output()
        .expect("run runs");
    assert!(output.status.success(), "{:?}", output.stderr);
    let value: Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    let items = value["data"]["items"].as_array().expect("run items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], 12);
    assert_eq!(items[0]["task_id"], 7);
    assert_eq!(items[0]["task_title"], "Populated task");
    assert_eq!(items[0]["duration_ms"], 2000);
    assert_eq!(items[0]["outcome"], "completed");
}

#[test]
fn scan_imports_orca_structured_tool_calls_once_through_the_cli() {
    let home = tempfile::tempdir().expect("temporary home");
    let state = home.path().join("orca");
    std::fs::create_dir_all(&state).expect("create Orca state directory");
    let journal = state.join("agent-session-journal.db");
    let source = rusqlite::Connection::open(&journal).expect("create journal");
    source.execute_batch(
        "CREATE TABLE journal_sessions (session_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, epoch TEXT NOT NULL);
         CREATE TABLE journal_rows (session_id TEXT NOT NULL, epoch TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL, row_json TEXT NOT NULL, PRIMARY KEY(session_id, epoch, seq));
         PRAGMA user_version = 4;
         INSERT INTO journal_sessions VALUES ('session-a', 'workspace-a', 'epoch-a');",
    ).expect("create journal schema");
    let epoch = serde_json::json!({
        "v": 3,
        "epoch": "epoch-a",
        "seq": 1,
        "fence": 0,
        "ts": 1_767_225_601_000i64,
        "kind": "epoch",
        "reason": "session_created",
        "providerHandle": {"kind": "codex", "threadId": "thread-a"}
    });
    source
        .execute(
            "INSERT INTO journal_rows VALUES ('session-a', 'epoch-a', 1, 1767225601000, ?1)",
            [epoch.to_string()],
        )
        .expect("insert epoch row");
    for (sequence, revision, state, output) in [
        (2, 1, "running", serde_json::Value::Null),
        (
            3,
            2,
            "completed",
            serde_json::json!({
                "head": "ok",
                "byteLength": 2,
                "digest": "sha256-cli-output",
                "truncated": false
            }),
        ),
    ] {
        let mut body = serde_json::json!({
            "kind": "tool-call",
            "name": "read",
            "callId": "call-cli",
            "state": state,
            "input": {"path": "README.md"}
        });
        if !output.is_null() {
            body["output"] = output;
        }
        let row = serde_json::json!({
            "v": 3,
            "epoch": "epoch-a",
            "seq": sequence,
            "fence": 0,
            "ts": 1_767_225_600_000i64 + sequence * 1_000,
            "kind": "item",
            "itemId": "tool-item-cli",
            "revision": revision,
            "body": body,
            "turnScope": {"kind": "turn", "turnItemId": "turn-a"}
        });
        source
            .execute(
                "INSERT INTO journal_rows VALUES ('session-a', 'epoch-a', ?1, ?2, ?3)",
                rusqlite::params![
                    sequence,
                    1_767_225_600_000i64 + sequence * 1_000,
                    row.to_string()
                ],
            )
            .expect("insert tool item revision");
    }
    drop(source);
    let database = home.path().join("orcalens.db");
    for expected in [3, 0] {
        let output = Command::new(env!("CARGO_BIN_EXE_orcalens"))
            .args(["scan", "--json"])
            .env("HOME", home.path())
            .env("ORCALENS_DB", &database)
            .env("ORCA_STATE_DIR", &state)
            .output()
            .expect("run scan");
        assert!(output.status.success(), "{:?}", output.stderr);
        let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON output");
        assert_eq!(value["data"]["events_imported"], expected);
    }
    let destination = rusqlite::Connection::open(database).expect("open analytics database");
    let (identity, payload): (String, String) = destination
        .query_row(
            "SELECT source_identity, payload FROM raw_events WHERE source_identity = 'session-a:epoch-a:1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read imported epoch row");
    assert_eq!(identity, "session-a:epoch-a:1");
    assert_eq!(
        serde_json::from_str::<Value>(&payload).unwrap()["kind"],
        "epoch"
    );
    let call: (String, String, Option<String>, Option<String>) = destination
        .query_row(
            "SELECT tool_name, status, input_hash, output_hash FROM tool_calls WHERE call_id = 'call-cli'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("normalized tool call");
    assert_eq!(call.0, "read");
    assert_eq!(call.1, "succeeded");
    assert_eq!(call.2.as_ref().map(String::len), Some(16));
    assert_eq!(call.3.as_deref(), Some("sha256-cli-output"));
}

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
    for command in ["scan", "runs", "skills", "waste", "report"] {
        assert!(
            commands
                .lines()
                .any(|line| line.split_whitespace().next() == Some(command)),
            "help should list {command}"
        );
    }
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
fn scan_imports_fixture_journal_once_through_the_cli() {
    let home = tempfile::tempdir().expect("temporary home");
    let state = home.path().join("orca");
    std::fs::create_dir_all(&state).expect("create Orca state directory");
    let journal = state.join("agent-session-journal.db");
    let source = rusqlite::Connection::open(&journal).expect("create journal");
    source.execute_batch(
        "CREATE TABLE journal_sessions (session_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, epoch TEXT NOT NULL);
         CREATE TABLE journal_rows (session_id TEXT NOT NULL, epoch TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL, row_json TEXT NOT NULL, PRIMARY KEY(session_id, epoch, seq));
         PRAGMA user_version = 4;
         INSERT INTO journal_sessions VALUES ('session-a', '/tmp', 'epoch-a');
         INSERT INTO journal_rows VALUES ('session-a', 'epoch-a', 1, 1767225600000, '{\"type\":\"message\"}');",
    ).expect("write journal fixture");
    drop(source);
    let database = home.path().join("orcalens.db");
    for expected in [1, 0] {
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
            "SELECT source_identity, payload FROM raw_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read imported event");
    assert_eq!(identity, "session-a:epoch-a:1");
    assert_eq!(payload, "{\"type\":\"message\"}");
}

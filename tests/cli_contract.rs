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

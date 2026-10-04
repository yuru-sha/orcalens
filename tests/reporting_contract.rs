use orcalens::cli::Command;
use orcalens::reporting::{self, CommandData, CommandOutput, ListResult};

#[test]
fn json_output_uses_command_and_data_fields() {
    let output = CommandOutput {
        command: Command::Runs,
        data: CommandData::List(ListResult { items: Vec::new() }),
    };
    let mut bytes = Vec::new();
    reporting::write(&mut bytes, &output, true).expect("write JSON");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("parse JSON");
    assert_eq!(value["command"], "runs");
    assert_eq!(value["data"]["items"], serde_json::json!([]));
}

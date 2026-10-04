use crate::cli::Command;
use serde::Serialize;
use std::io::{self, Write};

#[derive(Serialize)]
pub struct CommandOutput {
    pub command: Command,
    pub data: CommandData,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum CommandData {
    Scan(ScanResult),
    List(ListResult),
    Report(ReportResult),
}

#[derive(Serialize)]
pub struct ScanResult {
    pub sources_scanned: u64,
    pub events_imported: u64,
    pub collectors_available: u64,
}

#[derive(Serialize)]
pub struct ListResult {
    pub items: Vec<String>,
}

#[derive(Serialize)]
pub struct ReportResult {
    pub runs: i64,
    pub skills: i64,
    pub findings: i64,
}

impl CommandOutput {
    pub fn empty_list(command: Command) -> Self {
        Self {
            command,
            data: CommandData::List(ListResult { items: Vec::new() }),
        }
    }
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
        CommandData::Scan(result) => format!(
            "No collectors are configured. Scanned {} sources and imported {} events.",
            result.sources_scanned, result.events_imported
        ),
        CommandData::List(result) if result.items.is_empty() => {
            let label = match output.command {
                Command::Runs => "run",
                Command::Skills => "skill",
                Command::Waste => "waste finding",
                Command::Scan | Command::Report => "record",
            };
            format!("No {label} data is available.")
        }
        CommandData::List(result) => result.items.join("\n"),
        CommandData::Report(result) => format!(
            "Stored data: {} runs, {} skills, {} findings.",
            result.runs, result.skills, result.findings
        ),
    }
}

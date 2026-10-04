use crate::cli::Command;
use serde::Serialize;
use std::io::{self, Write};

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
    let name = match command {
        Command::Scan => "scan",
        Command::Runs => "runs",
        Command::Skills { .. } => "skills",
        Command::Waste => "waste",
        Command::Report => "report",
    };
    serializer.serialize_str(name)
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum CommandData {
    Scan(ScanResult),
    Skills(SkillsResult),
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
pub struct SkillResult {
    pub name: String,
    pub installed: bool,
    pub providers: Vec<String>,
    pub calls: i64,
    pub runs: i64,
    pub last_used: Option<String>,
    pub inactive: bool,
}

#[derive(Serialize)]
pub struct SkillsResult {
    pub items: Vec<SkillResult>,
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
        CommandData::Skills(result) if result.items.is_empty() => {
            "No skill data is available.".to_owned()
        }
        CommandData::Skills(result) => result
            .items
            .iter()
            .map(|skill| {
                format!(
                    "{}: {} calls across {} runs{}",
                    skill.name,
                    skill.calls,
                    skill.runs,
                    if skill.installed {
                        " (installed)"
                    } else {
                        " (not installed)"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        CommandData::Scan(result) => format!(
            "Scanned {} sources and imported {} events.",
            result.sources_scanned, result.events_imported
        ),
        CommandData::List(result) if result.items.is_empty() => {
            let label = match output.command {
                Command::Runs => "run",
                Command::Skills { .. } => "skill",
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

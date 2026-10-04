use crate::reporting::{
    self, CommandData, CommandOutput, ReportResult, ScanResult, SkillResult, SkillsResult,
};
use crate::storage;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::error::Error;
use std::ffi::OsString;
use std::io;

#[derive(Parser)]
#[command(name = "orcalens", version, about = "Analyze Orca agent runs")]
struct Arguments {
    #[arg(long, global = true, help = "Write the result as JSON to stdout")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, Serialize, Subcommand)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Scan,
    Runs,
    Skills {
        #[arg(
            long,
            help = "Show only skills unused or inactive for the configured window"
        )]
        unused: bool,
        #[arg(long, default_value_t = 90, value_name = "DAYS")]
        inactivity_days: u64,
    },
    Waste,
    Report,
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse_from(args);
    let mut connection = storage::open_default()?;
    let output = match arguments.command {
        Command::Scan => {
            let summary = crate::collectors::scan(&mut connection)?;
            CommandOutput {
                command: arguments.command,
                data: CommandData::Scan(ScanResult {
                    sources_scanned: summary.sources_scanned,
                    events_imported: summary.events_imported,
                    collectors_available: 1,
                }),
            }
        }
        Command::Runs => CommandOutput::empty_list(arguments.command),
        Command::Skills {
            unused,
            inactivity_days,
        } => {
            let items = crate::skills::refresh(&mut connection, inactivity_days)?
                .into_iter()
                .filter(|skill| !unused || skill.calls == 0 || skill.inactive)
                .map(|skill| SkillResult {
                    name: skill.name,
                    installed: skill.installed,
                    providers: skill.providers,
                    calls: skill.calls,
                    runs: skill.runs,
                    last_used: skill.last_used,
                    inactive: skill.inactive,
                })
                .collect();
            CommandOutput {
                command: Command::Skills {
                    unused,
                    inactivity_days,
                },
                data: CommandData::Skills(SkillsResult { items }),
            }
        }
        Command::Waste => CommandOutput::empty_list(Command::Waste),
        Command::Report => CommandOutput {
            command: arguments.command,
            data: CommandData::Report(ReportResult {
                runs: count_rows(&connection, "runs")?,
                skills: count_rows(&connection, "skill_inventory")?,
                findings: count_rows(&connection, "findings")?,
            }),
        },
    };
    reporting::write(io::stdout().lock(), &output, arguments.json)?;
    Ok(())
}

fn count_rows(
    connection: &rusqlite::Connection,
    table: &'static str,
) -> Result<i64, rusqlite::Error> {
    let query = match table {
        "runs" => "SELECT COUNT(*) FROM runs",
        "skill_inventory" => "SELECT COUNT(*) FROM skill_inventory",
        "findings" => "SELECT COUNT(*) FROM findings",
        _ => unreachable!(),
    };
    connection.query_row(query, [], |row| row.get(0))
}

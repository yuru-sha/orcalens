use crate::reporting::{self, CommandData, CommandOutput, ReportRequest, ReportView};
use crate::storage;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::{error::Error, ffi::OsString, io};

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
    Waste {
        #[arg(
            long,
            default_value_t = 90,
            value_name = "DAYS",
            help = "Report skills idle relative to latest stored evidence (default: 90)"
        )]
        inactivity_days: u64,
        #[arg(long, default_value_t = 2, value_name = "COUNT", value_parser = clap::value_parser!(u64).range(2..), help = "Minimum repeated calls (default: 2)")]
        repeat_count: u64,
        #[arg(long, default_value_t = 2, value_name = "MULTIPLIER", value_parser = clap::value_parser!(u64).range(2..), help = "Report runs at least this multiple of their task's historical median duration (default: 2; 3 prior runs required)")]
        long_run_multiplier: u64,
    },
    Report,
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse_from(args);
    let mut connection = storage::open_default()?;
    let output = match arguments.command {
        Command::Scan => CommandOutput {
            command: arguments.command,
            data: CommandData::Scan(reporting::scan(&mut connection)?),
        },
        Command::Runs => {
            let snapshot = reporting::query(&connection, &ReportRequest::new(ReportView::Runs))?;
            let data = match snapshot.view {
                reporting::ReportData::Runs(result) => result,
                _ => unreachable!(),
            };
            CommandOutput {
                command: arguments.command,
                data: CommandData::List(data),
            }
        }
        Command::Skills {
            unused,
            inactivity_days,
        } => {
            let mut result = reporting::refresh_skills(&mut connection, inactivity_days)?;
            if unused {
                result.items.retain(|skill| {
                    matches!(
                        skill.status,
                        reporting::SkillStatus::NeverUsed | reporting::SkillStatus::Dormant
                    )
                });
            }
            CommandOutput {
                command: arguments.command,
                data: CommandData::Skills(result),
            }
        }
        Command::Waste {
            inactivity_days,
            repeat_count,
            long_run_multiplier,
        } => {
            let mut request = ReportRequest::new(ReportView::Waste);
            request.thresholds = crate::analyzers::WasteThresholds {
                inactivity_days,
                repeat_count,
                long_run_multiplier,
            };
            let snapshot = reporting::query(&connection, &request)?;
            let items = match snapshot.view {
                reporting::ReportData::Waste(result) => result.items,
                _ => unreachable!(),
            };
            CommandOutput {
                command: arguments.command,
                data: CommandData::Waste(reporting::WasteResult { items }),
            }
        }
        Command::Report => {
            let snapshot = reporting::query(&connection, &ReportRequest::new(ReportView::Summary))?;
            let result = match snapshot.view {
                reporting::ReportData::Summary(result) => result,
                _ => unreachable!(),
            };
            CommandOutput {
                command: arguments.command,
                data: CommandData::Report(result),
            }
        }
    };
    reporting::write(io::stdout().lock(), &output, arguments.json)?;
    Ok(())
}

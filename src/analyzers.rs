use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::error::Error;

#[derive(Clone, Copy, Debug)]
pub struct WasteThresholds {
    pub inactivity_days: u64,
    pub repeat_count: u64,
    pub long_run_multiplier: u64,
}

impl Default for WasteThresholds {
    fn default() -> Self {
        Self {
            inactivity_days: 90,
            repeat_count: 2,
            long_run_multiplier: 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    NeverInvokedSkill,
    InactiveSkill,
    RepeatedSkillCall,
    RepeatedIdenticalToolCall,
    FailedToolCall,
    LongRun,
    InterruptedRun,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Evidence {
    pub entity_type: String,
    pub id: i64,
    pub observed_at: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct AffectedEntity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Metric {
    pub name: String,
    pub value: i64,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Threshold {
    pub value: i64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub finding_type: FindingKind,
    pub evidence: Vec<Evidence>,
    pub affected: AffectedEntity,
    pub metric: Metric,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<Threshold>,
    pub explanation: String,
}

pub fn analyze(
    connection: &Connection,
    thresholds: WasteThresholds,
) -> Result<Vec<Finding>, Box<dyn Error>> {
    let mut findings = Vec::new();
    let evidence_clock = latest_evidence(connection)?;
    let idle_cutoff = evidence_clock.as_ref().map(|(timestamp, _)| {
        timestamp.saturating_sub(
            i64::try_from(thresholds.inactivity_days)
                .unwrap_or(i64::MAX)
                .saturating_mul(86_400_000),
        )
    });
    {
        let mut statement = connection.prepare(
            "SELECT inventory.skill_name, COUNT(DISTINCT c.id),
                    COUNT(DISTINCT CASE WHEN c.started_at != ''
                        AND c.started_at NOT GLOB '*[^0-9]*' THEN c.id END),
                    MAX(CASE WHEN c.started_at != '' AND c.started_at NOT GLOB '*[^0-9]*'
                        THEN CAST(c.started_at AS INTEGER) END)
             FROM skill_inventory inventory
             LEFT JOIN skill_calls c ON c.skill_name = inventory.skill_name
             GROUP BY inventory.skill_name
             ORDER BY inventory.skill_name",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        for row in rows {
            let (skill, calls, dated_calls, last_used) = row?;
            if calls == 0 {
                findings.push(Finding {
                    finding_type: FindingKind::NeverInvokedSkill,
                    evidence: inventory_evidence(connection, &skill)?,
                    affected: AffectedEntity {
                        skill: Some(skill.clone()),
                        ..Default::default()
                    },
                    metric: Metric {
                        name: "observed_calls".into(),
                        value: 0,
                        unit: "calls".into(),
                    },
                    threshold: Some(Threshold {
                        value: 0,
                        unit: "calls".into(),
                        baseline: Some("No matching stored skill_calls records".into()),
                    }),
                    explanation: format!("Installed skill '{skill}' has no matching invocation in the stored skill-call records. Inventory is a snapshot and may be stale."),
                });
            } else if calls == dated_calls {
                if let (Some(cutoff), Some(last)) = (idle_cutoff, last_used) {
                    if last <= cutoff {
                        let mut evidence = inventory_evidence(connection, &skill)?;
                        evidence.extend(skill_evidence(connection, &skill)?);
                        if let Some((_, latest)) = &evidence_clock {
                            evidence.push(latest.clone());
                        }
                        evidence.sort();
                        evidence.dedup();
                        findings.push(Finding {
                        finding_type: FindingKind::InactiveSkill,
                        evidence,
                        affected: AffectedEntity { skill: Some(skill.clone()), ..Default::default() },
                        metric: Metric { name: "days_since_last_observed_call".into(), value: evidence_clock.as_ref().map_or(0, |(timestamp, _)| timestamp.saturating_sub(last) / 86_400_000), unit: "days".into() },
                        threshold: Some(Threshold { value: i64::try_from(thresholds.inactivity_days).unwrap_or(i64::MAX), unit: "days".into(), baseline: evidence_clock.as_ref().map(|(_, latest)| format!("latest evidence {} {} at {}; not wall-clock time", latest.entity_type, latest.id, latest.observed_at.as_deref().unwrap_or_default())) }),
                        explanation: format!("Skill '{skill}' last appears in stored evidence at {last}; this is at least {} days before the latest stored evidence timestamp. This is not a claim about current use outside the collected data.", thresholds.inactivity_days),
                    });
                    }
                }
            }
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT run_id, skill_name, COUNT(*) AS calls
             FROM skill_calls WHERE run_id IS NOT NULL
             GROUP BY run_id, skill_name HAVING COUNT(*) >= ?1
             ORDER BY run_id, skill_name",
        )?;
        let rows = statement.query_map(
            [i64::try_from(thresholds.repeat_count).unwrap_or(i64::MAX)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?;
        for row in rows {
            let (run, skill, count) = row?;
            findings.push(Finding {
                finding_type: FindingKind::RepeatedSkillCall,
                evidence: skill_run_evidence(connection, run, &skill)?,
                affected: AffectedEntity { run_id: Some(run), skill: Some(skill.clone()), ..Default::default() },
                metric: Metric { name: "calls_in_run".into(), value: count, unit: "calls".into() },
                threshold: Some(Threshold { value: i64::try_from(thresholds.repeat_count).unwrap_or(i64::MAX), unit: "calls".into(), baseline: None }),
                explanation: format!("Skill '{skill}' was recorded {count} times in run {run}; repeated calls alone do not establish redundant work."),
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT run_id, session_id, tool_name, input_hash, COUNT(*)
             FROM tool_calls WHERE run_id IS NOT NULL AND input_hash IS NOT NULL
             GROUP BY run_id, session_id, tool_name, input_hash HAVING COUNT(*) >= ?1
             ORDER BY run_id, session_id, tool_name, input_hash",
        )?;
        let rows = statement.query_map(
            [i64::try_from(thresholds.repeat_count).unwrap_or(i64::MAX)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        for row in rows {
            let (run, session, tool, hash, count) = row?;
            findings.push(Finding {
                finding_type: FindingKind::RepeatedIdenticalToolCall,
                evidence: tool_group_evidence(connection, run, session, &tool, &hash)?,
                affected: AffectedEntity { run_id: Some(run), session_id: session, tool: Some(tool.clone()), ..Default::default() },
                metric: Metric { name: "calls_with_same_input_hash_in_run".into(), value: count, unit: "calls".into() },
                threshold: Some(Threshold { value: i64::try_from(thresholds.repeat_count).unwrap_or(i64::MAX), unit: "calls".into(), baseline: Some(format!("tool name and normalized input hash {hash}")) }),
                explanation: format!("Tool '{tool}' has {count} calls in run {run} with the same stored input hash. Hash equality is exact at the normalized hash level and does not prove the operations were unnecessary."),
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT id, run_id, session_id, tool_name, status, COALESCE(started_at, ended_at) FROM tool_calls WHERE status = 'failed' ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (id, run, session, tool, status, timestamp) = row?;
            findings.push(Finding {
                finding_type: FindingKind::FailedToolCall,
                evidence: vec![Evidence {
                    entity_type: "tool_call".into(),
                    id,
                    observed_at: timestamp,
                }],
                affected: AffectedEntity {
                    run_id: run,
                    session_id: session,
                    tool: Some(tool.clone()),
                    ..Default::default()
                },
                metric: Metric {
                    name: "failed_calls".into(),
                    value: 1,
                    unit: "call".into(),
                },
                threshold: Some(Threshold {
                    value: 1,
                    unit: "failed status".into(),
                    baseline: Some(status),
                }),
                explanation: format!("Stored tool call {id} for '{tool}' has status failed."),
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT id, started_at, ended_at, outcome FROM runs WHERE outcome = 'interrupted' ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, started, ended, outcome) = row?;
            findings.push(Finding {
                finding_type: FindingKind::InterruptedRun,
                evidence: vec![Evidence { entity_type: "run".into(), id, observed_at: ended.clone().or(started.clone()) }],
                affected: AffectedEntity { run_id: Some(id), ..Default::default() },
                metric: Metric { name: "interrupted_runs".into(), value: 1, unit: "run".into() },
                threshold: Some(Threshold { value: 1, unit: "interrupted outcome".into(), baseline: Some(outcome) }),
                explanation: format!("Run {id} has an explicit interrupted outcome in the normalized lifecycle evidence."),
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT id, task_id, started_at, ended_at
             FROM runs
             WHERE task_id IS NOT NULL AND started_at IS NOT NULL AND ended_at IS NOT NULL
               AND (outcome IS NULL OR outcome NOT IN ('interrupted', 'unverifiable'))
             ORDER BY task_id, CAST(started_at AS INTEGER), id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, task_id, started, ended) = row?;
            let (Ok(start), Ok(end)) = (started.parse::<i64>(), ended.parse::<i64>()) else {
                continue;
            };
            let duration = end.saturating_sub(start);
            if duration <= 0 {
                continue;
            }
            let Some((median, baseline_evidence, baseline_count)) =
                prior_task_duration_baseline(connection, task_id, id, start)?
            else {
                continue;
            };
            let multiplier = i64::try_from(thresholds.long_run_multiplier).unwrap_or(i64::MAX);
            if duration >= median.saturating_mul(multiplier) {
                let mut evidence = vec![Evidence {
                    entity_type: "run".into(),
                    id,
                    observed_at: Some(ended),
                }];
                evidence.extend(baseline_evidence);
                findings.push(Finding {
                    finding_type: FindingKind::LongRun,
                    evidence,
                    affected: AffectedEntity {
                        run_id: Some(id),
                        ..Default::default()
                    },
                    metric: Metric {
                        name: "duration".into(),
                        value: duration,
                        unit: "milliseconds".into(),
                    },
                    threshold: Some(Threshold {
                        value: multiplier,
                        unit: "times_median".into(),
                        baseline: Some(format!(
                            "median {median} milliseconds from {baseline_count} prior completed runs for task {task_id}; minimum baseline size is {MINIMUM_LONG_RUN_BASELINE}"
                        )),
                    }),
                    explanation: format!(
                        "Run {id} lasted {duration} ms, at least {} times the {median} ms median of {baseline_count} earlier completed runs for the same task.",
                        thresholds.long_run_multiplier
                    ),
                });
            }
        }
    }
    findings.sort_by(|a, b| {
        (
            &a.finding_type,
            &a.affected.run_id,
            &a.affected.skill,
            &a.affected.tool,
            &a.evidence,
        )
            .cmp(&(
                &b.finding_type,
                &b.affected.run_id,
                &b.affected.skill,
                &b.affected.tool,
                &b.evidence,
            ))
    });
    Ok(findings)
}

fn latest_evidence(connection: &Connection) -> Result<Option<(i64, Evidence)>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT CAST(timestamp AS INTEGER), entity_type, id, timestamp
             FROM (
                 SELECT 'run' AS entity_type, id, started_at AS timestamp FROM runs WHERE started_at IS NOT NULL
                 UNION ALL SELECT 'run', id, ended_at FROM runs WHERE ended_at IS NOT NULL
                 UNION ALL SELECT 'tool_call', id, started_at FROM tool_calls WHERE started_at IS NOT NULL
                 UNION ALL SELECT 'tool_call', id, ended_at FROM tool_calls WHERE ended_at IS NOT NULL
                 UNION ALL SELECT 'skill_call', id, started_at FROM skill_calls WHERE started_at IS NOT NULL
                 UNION ALL SELECT 'skill_call', id, ended_at FROM skill_calls WHERE ended_at IS NOT NULL
             )
             WHERE timestamp != '' AND timestamp NOT GLOB '*[^0-9]*'
             ORDER BY CAST(timestamp AS INTEGER) DESC, entity_type, id, timestamp DESC
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    Evidence {
                        entity_type: row.get(1)?,
                        id: row.get(2)?,
                        observed_at: Some(row.get(3)?),
                    },
                ))
            },
        )
        .optional()
}

fn inventory_evidence(
    connection: &Connection,
    skill: &str,
) -> Result<Vec<Evidence>, rusqlite::Error> {
    let mut statement =
        connection.prepare("SELECT id FROM skill_inventory WHERE skill_name = ?1 ORDER BY id")?;
    let rows = statement.query_map([skill], |row| {
        Ok(Evidence {
            entity_type: "skill_inventory".into(),
            id: row.get(0)?,
            observed_at: None,
        })
    })?;
    rows.collect()
}

fn skill_evidence(connection: &Connection, skill: &str) -> Result<Vec<Evidence>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT id, started_at FROM skill_calls WHERE skill_name = ?1 ORDER BY started_at, id",
    )?;
    let rows = statement.query_map([skill], |row| {
        Ok(Evidence {
            entity_type: "skill_call".into(),
            id: row.get(0)?,
            observed_at: row.get(1)?,
        })
    })?;
    rows.collect()
}

const MINIMUM_LONG_RUN_BASELINE: usize = 3;

fn skill_run_evidence(
    connection: &Connection,
    run_id: i64,
    skill: &str,
) -> Result<Vec<Evidence>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT id, started_at FROM skill_calls WHERE run_id = ?1 AND skill_name = ?2 ORDER BY id",
    )?;
    let rows = statement.query_map(rusqlite::params![run_id, skill], |row| {
        Ok(Evidence {
            entity_type: "skill_call".into(),
            id: row.get(0)?,
            observed_at: row.get(1)?,
        })
    })?;
    rows.collect()
}

fn tool_group_evidence(
    connection: &Connection,
    run_id: i64,
    session_id: Option<i64>,
    tool: &str,
    input_hash: &str,
) -> Result<Vec<Evidence>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT id, COALESCE(started_at, ended_at) FROM tool_calls
         WHERE run_id = ?1 AND session_id IS ?2 AND tool_name = ?3 AND input_hash = ?4
         ORDER BY id",
    )?;
    let rows = statement.query_map(
        rusqlite::params![run_id, session_id, tool, input_hash],
        |row| {
            Ok(Evidence {
                entity_type: "tool_call".into(),
                id: row.get(0)?,
                observed_at: row.get(1)?,
            })
        },
    )?;
    rows.collect()
}

fn prior_task_duration_baseline(
    connection: &Connection,
    task_id: i64,
    excluded_run_id: i64,
    candidate_start: i64,
) -> Result<Option<(i64, Vec<Evidence>, usize)>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT id, started_at, ended_at FROM runs
         WHERE task_id = ?1 AND id <> ?2 AND started_at IS NOT NULL AND ended_at IS NOT NULL
           AND CAST(started_at AS INTEGER) < ?3
           AND CAST(ended_at AS INTEGER) <= ?3
           AND (outcome IS NULL OR outcome NOT IN ('interrupted', 'unverifiable'))
         ORDER BY CAST(started_at AS INTEGER), id",
    )?;
    let rows = statement.query_map(
        rusqlite::params![task_id, excluded_run_id, candidate_start],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut evidence = Vec::new();
    let mut durations = Vec::new();
    for row in rows {
        let (id, started, ended) = row?;
        let (Ok(start), Ok(end)) = (started.parse::<i64>(), ended.parse::<i64>()) else {
            continue;
        };
        let duration = end.saturating_sub(start);
        if duration <= 0 {
            continue;
        }
        durations.push(duration);
        evidence.push(Evidence {
            entity_type: "run".into(),
            id,
            observed_at: Some(ended),
        });
    }
    if durations.len() < MINIMUM_LONG_RUN_BASELINE {
        return Ok(None);
    }
    durations.sort_unstable();
    let middle = durations.len() / 2;
    let median = if durations.len() % 2 == 0 {
        let lower = durations[middle - 1];
        lower + (durations[middle] - lower) / 2
    } else {
        durations[middle]
    };
    Ok(Some((median, evidence, durations.len())))
}

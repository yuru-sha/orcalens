use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct SkillUsage {
    pub name: String,
    pub installed: bool,
    pub providers: Vec<String>,
    pub calls: i64,
    pub runs: i64,
    pub last_used: Option<String>,
    pub inactive: bool,
}

pub fn refresh(
    connection: &mut Connection,
    inactivity_days: u64,
) -> Result<Vec<SkillUsage>, Box<dyn Error>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let roots = [
        ("omp", home.join(".agents/skills")),
        ("codex", home.join(".agents/skills")),
        ("codex", home.join(".codex/skills")),
        ("claude", home.join(".claude/skills")),
        ("opencode", home.join(".config/opencode/skills")),
        ("opencode", home.join(".local/share/opencode/skills")),
    ];
    let tx = connection.transaction()?;
    tx.execute("DELETE FROM skill_inventory", [])?;
    for (provider, root) in roots {
        for (name, path) in installed_skills(&root)? {
            tx.execute(
                "INSERT OR IGNORE INTO skill_inventory(skill_name,source,path) VALUES (?1,?2,?3)",
                rusqlite::params![name, provider, path.to_string_lossy()],
            )?;
        }
    }
    detect_events(&tx)?;
    let mut inventory: BTreeMap<String, SkillUsage> = BTreeMap::new();
    {
        let mut q =
            tx.prepare("SELECT skill_name,source FROM skill_inventory ORDER BY skill_name")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (name, source) = row?;
            let usage = inventory.entry(name.clone()).or_insert(SkillUsage {
                name,
                installed: true,
                providers: vec![],
                calls: 0,
                runs: 0,
                last_used: None,
                inactive: true,
            });
            if !usage.providers.contains(&source) {
                usage.providers.push(source);
            }
        }
    }
    {
        let mut q=tx.prepare("SELECT skill_name,COUNT(*),COUNT(DISTINCT run_id),MAX(started_at) FROM skill_calls GROUP BY skill_name")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })? {
            let (name, calls, runs, last) = row?;
            let usage = inventory.entry(name.clone()).or_insert(SkillUsage {
                name,
                installed: false,
                providers: vec![],
                calls: 0,
                runs: 0,
                last_used: None,
                inactive: true,
            });
            usage.calls = calls;
            usage.runs = runs;
            usage.last_used = last;
        }
    }
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .saturating_sub(u128::from(inactivity_days) * 86_400_000)
        .to_string();
    for usage in inventory.values_mut() {
        usage.inactive = usage.last_used.as_ref().is_none_or(|last| last < &cutoff);
    }
    tx.commit()?;
    Ok(inventory.into_values().collect())
}

fn installed_skills(root: &Path) -> Result<Vec<(String, PathBuf)>, Box<dyn Error>> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(found);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            let name = fs::read_to_string(path.join("SKILL.md"))
                .ok()
                .and_then(|text| frontmatter_name(&text))
                .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
            found.push((name, path));
        }
    }
    Ok(found)
}
fn frontmatter_name(text: &str) -> Option<String> {
    let body = text.strip_prefix("---")?.split_once("---")?.0;
    body.lines()
        .find_map(|line| {
            line.strip_prefix("name:")
                .map(|v| v.trim().trim_matches(['\"', '\'']).to_owned())
        })
        .filter(|s| !s.is_empty())
}

fn detect_events(tx: &rusqlite::Transaction<'_>) -> Result<(), Box<dyn Error>> {
    let mut q=tx.prepare("SELECT r.id,r.source_identity,r.source_timestamp,r.payload FROM raw_events r JOIN sources s ON s.id=r.source_id WHERE s.source_type='orca_journal' ORDER BY r.id")?;
    let events = q
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(q);
    for (event_id, identity, timestamp, payload) in events {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        let Some((session, epoch)) = identity
            .rsplit_once(':')
            .and_then(|(prefix, seq)| seq.parse::<i64>().ok().map(|_| prefix))
            .and_then(|prefix| prefix.rsplit_once(':'))
        else {
            continue;
        };
        let provider_row:Option<String>=tx.query_row("SELECT COALESCE(NULLIF(json_extract(payload,'$.providerHandle.kind'),'opaque'),json_extract(payload,'$.providerHandle.agent')) FROM raw_events WHERE source_identity = ?1",[format!("{session}:{epoch}:1")],|r|r.get(0)).ok();
        let provider = provider_row.or_else(|| {
            row.get("providerHandle")
                .and_then(|v| v.get("agent"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
        let Some(provider) = provider else { continue };
        let bodies: Vec<(&str, &serde_json::Value)> =
            if row.get("kind").and_then(|v| v.as_str()) == Some("item") {
                row.get("body")
                    .map(|b| vec![(row.get("itemId").and_then(|v| v.as_str()).unwrap_or(""), b)])
                    .unwrap_or_default()
            } else {
                row.get("mutations")
                    .and_then(|v| v.as_array())
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| Some((x.get("itemId")?.as_str()?, x.get("body")?)))
                            .collect()
                    })
                    .unwrap_or_default()
            };
        for (item_id, body) in bodies {
            let Some(name) = detect(&provider, body) else {
                continue;
            };
            let dedup = format!("{provider}:{session}:{epoch}:{item_id}:{name}");
            let turn = row
                .get("turnScope")
                .and_then(|v| v.get("turnItemId"))
                .and_then(|v| v.as_str());
            let run_id: Option<i64> = turn.and_then(|turn| {
                tx.query_row(
                    "SELECT runs.id FROM runs JOIN run_sessions ON run_sessions.run_id=runs.id WHERE runs.turn_item_id=?1 AND run_sessions.session_id=(SELECT id FROM sessions WHERE provider_session_id=(SELECT CAST(source_id AS TEXT)||':'||?2 FROM raw_events WHERE id=?3)) ORDER BY runs.id DESC LIMIT 1",
                    rusqlite::params![turn, session, event_id],
                    |r| r.get(0),
                )
                .ok()
            });
            tx.execute("INSERT INTO skill_calls(run_id,session_id,skill_name,source,started_at,source_event_id,dedup_key) VALUES (?1,(SELECT id FROM sessions WHERE provider_session_id=(SELECT CAST(source_id AS TEXT)||':'||?2 FROM raw_events WHERE id=?6)),?3,?4,?5,?6,?7) ON CONFLICT(dedup_key) DO NOTHING",rusqlite::params![run_id,session,name,provider,timestamp,event_id,dedup])?;
        }
    }
    Ok(())
}
pub trait SkillInvocationDetector {
    fn provider(&self) -> &'static str;
    fn detect(&self, body: &serde_json::Value) -> Option<String>;
}
pub struct CodexSkillDetector;
pub struct OmpSkillDetector;
pub struct ClaudeSkillDetector;
pub struct OpenCodeSkillDetector;
macro_rules! detector {
    ($type:ty, $provider:literal) => {
        impl SkillInvocationDetector for $type {
            fn provider(&self) -> &'static str {
                $provider
            }
            fn detect(&self, body: &serde_json::Value) -> Option<String> {
                detect_for($provider, body)
            }
        }
    };
}
detector!(CodexSkillDetector, "codex");
detector!(OmpSkillDetector, "omp");
detector!(ClaudeSkillDetector, "claude");
detector!(OpenCodeSkillDetector, "opencode");

fn detect(provider: &str, body: &serde_json::Value) -> Option<String> {
    let adapters: [&dyn SkillInvocationDetector; 4] = [
        &CodexSkillDetector,
        &OmpSkillDetector,
        &ClaudeSkillDetector,
        &OpenCodeSkillDetector,
    ];
    adapters
        .into_iter()
        .find(|adapter| adapter.provider() == provider)?
        .detect(body)
}
fn detect_for(provider: &str, body: &serde_json::Value) -> Option<String> {
    if body.get("kind").and_then(|v| v.as_str()) == Some("message")
        && body.get("role").and_then(|v| v.as_str()) == Some("user")
    {
        let text = body
            .get("blocks")?
            .as_array()?
            .iter()
            .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if text.trim_start().starts_with("<command-name>") {
            let value = text
                .split_once("<command-name>")?
                .1
                .split_once("</command-name>")?
                .0
                .trim()
                .trim_start_matches('/');
            if value.contains(':') {
                return value
                    .rsplit(':')
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
            }
        }
    }
    if body.get("kind").and_then(|v| v.as_str()) == Some("tool-call") {
        let tool = body.get("name")?.as_str()?;
        if provider == "claude" && tool.eq_ignore_ascii_case("Skill") {
            return body.get("input")?.get("skill")?.as_str().map(str::to_owned);
        }
        if provider == "opencode" && tool.eq_ignore_ascii_case("skill") {
            return body.get("input")?.get("name")?.as_str().map(str::to_owned);
        }
        if provider == "omp" && tool.to_lowercase().contains("skill") {
            return body.get("input")?.get("name")?.as_str().map(str::to_owned);
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaced_plugin_envelope_extracts_short_skill_name() {
        let body = serde_json::json!({"kind":"message","role":"user","blocks":[{"type":"text","text":"<command-name>/plugin:review</command-name>"}]});
        assert_eq!(detect("codex", &body).as_deref(), Some("review"));
    }

    #[test]
    fn provider_skill_tool_shapes_are_adapter_specific() {
        let claude =
            serde_json::json!({"kind":"tool-call","name":"Skill","input":{"skill":"deploy"}});
        assert_eq!(detect("claude", &claude).as_deref(), Some("deploy"));
        assert_eq!(detect("codex", &claude), None);
    }

    #[test]
    fn installed_skill_name_uses_frontmatter() {
        assert_eq!(
            frontmatter_name("---\nname: my-skill\ndescription: x\n---\n"),
            Some("my-skill".into())
        );
        assert_eq!(frontmatter_name("no frontmatter"), None);
    }

    #[test]
    fn duplicate_envelope_revisions_count_as_one_call() {
        let mut connection = crate::storage::open(":memory:").expect("open database");
        connection.execute("INSERT INTO sources(source_type,source_path,source_identity) VALUES ('orca_journal','journal','journal')", []).expect("insert source");
        let source_id: i64 = connection
            .query_row("SELECT id FROM sources", [], |row| row.get(0))
            .unwrap();
        let inserts = [
            (
                "session:epoch:1",
                serde_json::json!({"kind":"epoch","epoch":"epoch","seq":1,"providerHandle":{"kind":"codex","threadId":"thread"}}),
            ),
            (
                "session:epoch:2",
                serde_json::json!({"kind":"item","epoch":"epoch","seq":2,"itemId":"item-a","body":{"kind":"message","role":"user","blocks":[{"type":"text","text":"<command-name>/plugin:review</command-name>"}]}}),
            ),
            (
                "session:epoch:3",
                serde_json::json!({"kind":"item","epoch":"epoch","seq":3,"itemId":"item-a","body":{"kind":"message","role":"user","blocks":[{"type":"text","text":"<command-name>/plugin:review</command-name>"}]}}),
            ),
        ];
        for (identity, payload) in inserts {
            connection.execute("INSERT INTO raw_events(source_id,source_identity,observed_at,source_timestamp,payload,payload_hash) VALUES (?1,?2,'now','1767225600000',?3,'hash')", rusqlite::params![source_id, identity, payload.to_string()]).expect("insert journal event");
        }
        let tx = connection.transaction().unwrap();
        detect_events(&tx).expect("detect invocations");
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM skill_calls WHERE skill_name='review'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}

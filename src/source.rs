use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use crate::domain::record::Record;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    Absent(String),
    Unreadable(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent(p) => write!(f, "absent: {}", p),
            Self::Unreadable(e) => write!(f, "unreadable: {}", e),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStatus {
    pub name: String,
    pub record_count: usize,
    pub error: Option<SourceError>,
}

pub type SourceStatuses = Vec<SourceStatus>;

pub trait Source: Send + Sync {
    fn name(&self) -> &str;
    fn load(&self) -> Result<Vec<Record>, SourceError>;
}

pub struct Registry {
    sources: BTreeMap<String, Box<dyn Source>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { sources: BTreeMap::new() }
    }
    pub fn register(&mut self, source: Box<dyn Source>) {
        self.sources.insert(source.name().to_string(), source);
    }
    pub fn load_all(&self) -> (Vec<Record>, SourceStatuses) {
        let mut records = Vec::new();
        let mut statuses = Vec::new();
        for (name, src) in &self.sources {
            match src.load() {
                Ok(batch) => {
                    let count = batch.len();
                    records.extend(batch);
                    statuses.push(SourceStatus {
                        name: name.clone(),
                        record_count: count,
                        error: None,
                    });
                }
                Err(e) => {
                    statuses.push(SourceStatus {
                        name: name.clone(),
                        record_count: 0,
                        error: Some(e),
                    });
                }
            }
        }
        (records, statuses)
    }
    pub fn source_names(&self) -> Vec<&str> {
        self.sources.keys().map(|s| s.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// Claude adapter
// ---------------------------------------------------------------------------

mod claude {
    use super::*;
    use crate::domain::record::{Record, TokenBreakdown};
    use chrono::{DateTime, Utc};

    #[derive(serde::Deserialize, Debug)]
    struct Line {
        #[serde(rename = "type")]
        line_type: Option<String>,
        timestamp: Option<String>,
        message: Option<MessageInner>,
    }

    #[derive(serde::Deserialize, Debug)]
    struct MessageInner {
        model: Option<String>,
        usage: Option<Usage>,
    }

    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    }

    pub struct ClaudeSource {
        project_dir: PathBuf,
    }

    impl ClaudeSource {
        pub fn new(project_dir: PathBuf) -> Self {
            Self { project_dir }
        }

        /// Decode a hyphen-encoded project directory name into a path.
        /// Claude encodes `/home/foo/bar` as `-home-foo-bar`.
        /// The leading `-` represents the leading `/` of the absolute path.
        pub(crate) fn decode_project_name(dir_name: &str) -> String {
            let without_prefix = dir_name.strip_prefix('-').unwrap_or(dir_name);
            let decoded: String = without_prefix
                .chars()
                .flat_map(|c| if c == '-' { vec!['/'] } else { vec![c] })
                .collect();
            // If the original started with '-', the path was absolute — prepend '/'.
            if dir_name.starts_with('-') && !decoded.is_empty() && !decoded.starts_with('/') {
                format!("/{}", decoded)
            } else {
                decoded
            }
        }

        fn parse_timestamp(ts: &str) -> Option<DateTime<Utc>> {
            DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        }
    }

    impl Source for ClaudeSource {
        fn name(&self) -> &str { "claude" }

        fn load(&self) -> Result<Vec<Record>, SourceError> {
            if !self.project_dir.exists() {
                return Err(SourceError::Absent(self.project_dir.to_string_lossy().to_string()));
            }
            let mut records = Vec::new();
            let entries = match self.project_dir.read_dir() {
                Ok(e) => e,
                Err(e) => return Err(SourceError::Unreadable(e.to_string())),
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let project = Self::decode_project_name(
                    dir.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                );
                let dir_entries = match std::fs::read_dir(&dir) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for de in dir_entries.filter_map(|e| e.ok()) {
                    let p = de.path();
                    if !p.is_file() {
                        continue;
                    }
                    let fname = match p.file_name().and_then(|n| n.to_str()) {
                        Some(n) if n.ends_with(".jsonl") => n.to_string(),
                        _ => continue,
                    };
                    let content = match std::fs::read_to_string(&p) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("warn: cannot read {:?}: {}", p, e);
                            continue;
                        }
                    };
                    let mut messages: Vec<(DateTime<Utc>, String, Usage)> = Vec::new();
                    for line in content.lines().filter(|l| !l.trim().is_empty()) {
                        let line: Line = match serde_json::from_str(line) {
                            Ok(l) => l,
                            Err(_) => continue,
                        };
                        if line.line_type.as_deref() != Some("assistant") {
                            continue;
                        }
                        let ts = line
                            .timestamp
                            .as_ref()
                            .and_then(|t| Self::parse_timestamp(t));
                        let model = line
                            .message
                            .as_ref()
                            .and_then(|m| m.model.clone());
                        let usage = line
                            .message
                            .as_ref()
                            .and_then(|m| m.usage.clone())
                            .unwrap_or_default();
                        if let Some(ts) = ts {
                            messages.push((ts, model.unwrap_or_default(), usage));
                        }
                    }
                    messages.sort_by_key(|(ts, _, _)| *ts);
                    let started_at = messages.first().map(|(ts, _, _)| *ts).unwrap_or_else(|| Utc::now());
                    let ended_at = messages.last().map(|(ts, _, _)| *ts);
                    let last_model = messages
                        .iter()
                        .rev()
                        .find_map(|(_, m, _)| {
                            if m.is_empty() {
                                None
                            } else {
                                Some(m.clone())
                            }
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    let tokens = TokenBreakdown {
                        input: messages
                            .iter()
                            .map(|(_, _, u)| u.input_tokens.unwrap_or(0))
                            .sum(),
                        output: messages
                            .iter()
                            .map(|(_, _, u)| u.output_tokens.unwrap_or(0))
                            .sum(),
                        reasoning: messages
                            .iter()
                            .map(|(_, _, u)| u.reasoning_tokens.unwrap_or(0))
                            .sum(),
                        cache_read: messages
                            .iter()
                            .map(|(_, _, u)| u.cache_read_input_tokens.unwrap_or(0))
                            .sum(),
                        cache_write: messages
                            .iter()
                            .map(|(_, _, u)| u.cache_creation_input_tokens.unwrap_or(0))
                            .sum(),
                    };
                    records.push(Record {
                        session_id: fname,
                        source: "claude".to_string(),
                        project: project.clone(),
                        model: last_model,
                        agent: None,
                        started_at,
                        ended_at,
                        tokens,
                        message_count: messages.len() as u32,
                        cost: None,
                    });
                }
            }
            Ok(records)
        }
    }
}

// ---------------------------------------------------------------------------
// OpenCode adapter
// ---------------------------------------------------------------------------

mod opencode {
    use super::*;
    use crate::domain::record::{Record, TokenBreakdown};
    use chrono::{DateTime, TimeZone, Utc};
    use rusqlite::Connection;

    pub struct OpenCodeSource {
        dbs: Vec<PathBuf>,
    }

    impl OpenCodeSource {
        pub fn new(dbs: Vec<PathBuf>) -> Self {
            Self { dbs }
        }
        fn normalize_model(raw: &str) -> String {
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
                    return id.to_string();
                }
            }
            raw.to_string()
        }
        fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
            Utc.timestamp_millis_opt(ms).single()
        }
    }

    impl Source for OpenCodeSource {
        fn name(&self) -> &str {
            "opencode"
        }

        fn load(&self) -> Result<Vec<Record>, SourceError> {
            if self.dbs.is_empty() {
                return Ok(Vec::new());
            }
            let mut seen: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut records = Vec::new();
            for db_path in &self.dbs {
                if !db_path.exists() {
                    continue;
                }
                let conn = match Connection::open(db_path) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("warn: cannot open {:?}: {}", db_path, e);
                        continue;
                    }
                };
                let mut stmt = match conn.prepare(
                    "SELECT id, model, agent, directory, title, cost,
                            tokens_input, tokens_output, tokens_reasoning,
                            tokens_cache_read, tokens_cache_write,
                            time_created, time_updated
                     FROM session"
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("warn: prepare failed for {:?}: {}", db_path, e);
                        continue;
                    }
                };
                let rows = match stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, i64>(12)?,
                    ))
                }) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("warn: query failed for {:?}: {}", db_path, e);
                        continue;
                    }
                };
                for row in rows {
                    let (
                        id, model_raw, agent, directory, _title,
                        cost, tokens_input, tokens_output, tokens_reasoning,
                        tokens_cache_read, tokens_cache_write,
                        time_created, time_updated,
                    ) = match row {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("warn: row error: {}", e);
                            continue;
                        }
                    };
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let started_at = match Self::ms_to_datetime(time_created) {
                        Some(t) => t,
                        None => continue, // skip rows with invalid timestamps
                    };
                    let ended_at = Self::ms_to_datetime(time_updated);
                    records.push(Record {
                        session_id: id,
                        source: "opencode".to_string(),
                        project: directory,
                        model: Self::normalize_model(&model_raw),
                        agent,
                        started_at,
                        ended_at,
                        tokens: TokenBreakdown {
                            input: tokens_input as u64,
                            output: tokens_output as u64,
                            reasoning: tokens_reasoning as u64,
                            cache_read: tokens_cache_read as u64,
                            cache_write: tokens_cache_write as u64,
                        },
                        message_count: 0,
                        cost: Some(cost),
                    });
                }
            }
            Ok(records)
        }
    }
}

pub use claude::ClaudeSource;
pub use opencode::OpenCodeSource;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::{Record, TokenBreakdown};
    use chrono::Utc;

    #[test]
    fn registry_loads_multiple_sources() {
        let mut reg = Registry::new();
        struct Fake {
            name: String,
            records: Vec<Record>,
        }
        impl Source for Fake {
            fn name(&self) -> &str {
                &self.name
            }
            fn load(&self) -> Result<Vec<Record>, SourceError> {
                Ok(self.records.clone())
            }
        }
        reg.register(Box::new(Fake {
            name: "a".to_string(),
            records: vec![
                Record {
                    session_id: "s1".into(),
                    source: "a".into(),
                    project: "/p".into(),
                    model: "m".into(),
                    agent: None,
                    started_at: Utc::now(),
                    ended_at: None,
                    tokens: TokenBreakdown::default(),
                    message_count: 1,
                    cost: None,
                },
                Record {
                    session_id: "s2".into(),
                    source: "a".into(),
                    project: "/p".into(),
                    model: "m".into(),
                    agent: None,
                    started_at: Utc::now(),
                    ended_at: None,
                    tokens: TokenBreakdown::default(),
                    message_count: 2,
                    cost: None,
                },
            ],
        }));
        reg.register(Box::new(Fake {
            name: "b".to_string(),
            records: vec![],
        }));
        let (records, statuses) = reg.load_all();
        assert_eq!(records.len(), 2);
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].record_count, 2);
        assert_eq!(statuses[1].record_count, 0);
    }

    #[test]
    fn registry_absent_source_returns_ok_empty() {
        struct AbsentSource;
        impl Source for AbsentSource {
            fn name(&self) -> &str {
                "missing"
            }
            fn load(&self) -> Result<Vec<Record>, SourceError> {
                Err(SourceError::Absent("/nonexistent".to_string()))
            }
        }
        let mut reg = Registry::new();
        reg.register(Box::new(AbsentSource));
        let (records, statuses) = reg.load_all();
        assert!(records.is_empty());
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].error.is_some());
        assert!(matches!(
            statuses[0].error.as_ref().unwrap(),
            SourceError::Absent(_)
        ));
    }

    #[test]
    fn decode_project_name_preserves_leading_slash() {
        use super::claude::ClaudeSource;
        assert_eq!(
            ClaudeSource::decode_project_name("-home-hunter-projects-modbox"),
            "/home/hunter/projects/modbox"
        );
        assert_eq!(
            ClaudeSource::decode_project_name("-single"),
            "/single"
        );
        assert_eq!(
            ClaudeSource::decode_project_name("no-prefix"),
            "no/prefix"
        );
    }
}

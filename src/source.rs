use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};


use chrono::{DateTime, Utc};
use crate::domain::record::{Record, TokenBreakdown};

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

/// Parse an RFC 3339 timestamp into UTC. Shared by all JSONL sources.
fn parse_timestamp(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Extract the recorded model id from a JSON model envelope.
/// OpenCode and Kilo Code both store `model` as `{"id":"auto","providerID":"freellm"}`
/// rather than a plain string; the `id` is the value the tool recorded, so it is
/// the grouping key. Non-envelope values are passed through raw.
fn normalize_model(raw: &str) -> String {
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
            return id.to_string();
        }
    }
    raw.to_string()
}

/// Recursively collect every `*.jsonl` file under `dir`. OMP nests subagent
/// sessions one directory level deeper than the project directory.
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match dir.read_dir() {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            collect_jsonl(&p, out);
        } else if p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".jsonl"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

/// Build a normalized `Record` from collected assistant events for one session.
/// `session_started_at`, when known, wins over the first message timestamp.
/// `cost`, if `Some`, is attached verbatim (sources that don't track cost pass
/// `None` so it stays absent rather than coerced to zero).
fn build_session_record(
    source: &str,
    session_id: String,
    project: String,
    session_started_at: Option<DateTime<Utc>>,
    events: &mut [(DateTime<Utc>, String, TokenBreakdown)],
    cost: Option<f64>,
) -> Option<Record> {
    if events.is_empty() {
        return None;
    }
    events.sort_by_key(|(ts, _, _)| *ts);
    let started_at = session_started_at
        .or_else(|| events.first().map(|(ts, _, _)| *ts))
        .unwrap_or_else(Utc::now);
    let ended_at = events.last().map(|(ts, _, _)| *ts);
    let last_model = events
        .iter()
        .rev()
        .find_map(|(_, m, _)| if m.is_empty() { None } else { Some(m.clone()) })
        .unwrap_or_else(|| "unknown".to_string());
    let mut tokens = TokenBreakdown::default();
    for (_, _, t) in events.iter() {
        tokens.add(t);
    }
    Some(Record {
        session_id,
        source: source.to_string(),
        project,
        model: last_model,
        agent: None,
        started_at,
        ended_at,
        tokens,
        message_count: events.len() as u32,
        cost,
    })
}

mod claude {
    use super::*;
    use crate::domain::record::TokenBreakdown;

    #[derive(serde::Deserialize, Debug)]
    struct Line {
        #[serde(rename = "type")]
        line_type: Option<String>,
        timestamp: Option<String>,
        message: Option<MessageInner>,
    }

    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct MessageInner {
        model: Option<String>,
        usage: Option<Usage>,
    }

    /// Real Claude Code transcripts nest reasoning + cache-creation under
    /// sub-objects rather than exposing flat fields. Both shapes are read so
    /// either form populates the breakdown.
    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct OutputTokensDetails {
        reasoning_tokens: Option<u64>,
    }

    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct CacheCreation {
        ephemeral_5m_input_tokens: Option<u64>,
        ephemeral_1h_input_tokens: Option<u64>,
    }

    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
        output_tokens_details: Option<OutputTokensDetails>,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        cache_creation: Option<CacheCreation>,
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
                let mut files = Vec::new();
                collect_jsonl(&dir, &mut files);
                for p in files {
                    let content = match std::fs::read_to_string(&p) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("warn: cannot read {:?}: {}", p, e);
                            continue;
                        }
                    };
                    let mut events: Vec<(DateTime<Utc>, String, TokenBreakdown)> = Vec::new();
                    for line in content.lines().filter(|l| !l.trim().is_empty()) {
                        let line: Line = match serde_json::from_str(line) {
                            Ok(l) => l,
                            Err(_) => continue,
                        };
                        if line.line_type.as_deref() != Some("assistant") {
                            continue;
                        }
                        let ts = match line.timestamp.as_deref().and_then(parse_timestamp) {
                            Some(ts) => ts,
                            None => continue,
                        };
                        let u = line
                            .message
                            .as_ref()
                            .and_then(|m| m.usage.clone())
                            .unwrap_or_default();
                        let reasoning = u
                            .reasoning_tokens
                            .or_else(|| {
                                u.output_tokens_details
                                    .as_ref()
                                    .and_then(|d| d.reasoning_tokens)
                            })
                            .unwrap_or(0);
                        // Reasoning tokens are folded into `output` — the
                        // canonical breakdown no longer tracks them separately.
                        let output = u.output_tokens.unwrap_or(0) + reasoning;
                        let cache_write = u
                            .cache_creation
                            .as_ref()
                            .map(|c| {
                                c.ephemeral_5m_input_tokens.unwrap_or(0)
                                    + c.ephemeral_1h_input_tokens.unwrap_or(0)
                            })
                            .or_else(|| u.cache_creation_input_tokens)
                            .unwrap_or(0);
                        let model = line
                            .message
                            .as_ref()
                            .and_then(|m| m.model.clone())
                            .unwrap_or_default();
                        events.push((
                            ts,
                            model,
                            TokenBreakdown {
                                input: u.input_tokens.unwrap_or(0),
                                output,
                                cache_read: u.cache_read_input_tokens.unwrap_or(0),
                                cache_write,
                            },
                        ));
                    }
                    if let Some(rec) = build_session_record(
                        "claude",
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string(),
                        project.clone(),
                        None,
                        &mut events,
                        None,
                    ) {
                        records.push(rec);
                    }
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
                                                model: normalize_model(&model_raw),
                        agent,
                        started_at,
                        ended_at,
                        tokens: TokenBreakdown {
                            input: tokens_input as u64,
                            output: (tokens_output + tokens_reasoning) as u64,
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

// ---------------------------------------------------------------------------
// OMP adapter
// ---------------------------------------------------------------------------
mod omp {
    use super::*;
    use crate::domain::record::TokenBreakdown;

    #[derive(serde::Deserialize, Debug)]
    struct Line {
        #[serde(rename = "type")]
        line_type: Option<String>,
        timestamp: Option<String>,
        /// Populated on `type == "session"` lines (flat, not nested).
        id: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        message: Option<MessageInner>,
    }

    #[derive(serde::Deserialize, Debug)]
    struct MessageInner {
        role: Option<String>,
        model: Option<String>,
        provider: Option<String>,
        usage: Option<Usage>,
    }

    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct Usage {
        input: Option<u64>,
        output: Option<u64>,
        #[serde(rename = "reasoningTokens")]
        reasoning_tokens: Option<u64>,
        #[serde(rename = "cacheRead")]
        cache_read: Option<u64>,
        #[serde(rename = "cacheWrite")]
        cache_write: Option<u64>,
        cost: Option<Cost>,
    }

    #[derive(serde::Deserialize, Debug, Default, Clone)]
    struct Cost {
        total: Option<f64>,
    }

    pub struct OmpSource {
        sessions_dir: PathBuf,
    }

    impl OmpSource {
        pub fn new(sessions_dir: PathBuf) -> Self {
            Self { sessions_dir }
        }

        /// Decode a hyphen-encoded project directory name.
        /// OMP encodes paths relative to $HOME: `~/projects/modbox` becomes
        /// `-projects-modbox`, and the home directory itself becomes `-`.
        pub(crate) fn decode_dir_name(dir_name: &str, home: &Path) -> String {
            let rest = dir_name.strip_prefix('-').unwrap_or(dir_name);
            if rest.is_empty() {
                return home.to_string_lossy().to_string();
            }
            let rel = rest.replace('-', "/");
            home.join(rel).to_string_lossy().to_string()
        }
    }

    impl Source for OmpSource {
        fn name(&self) -> &str { "omp" }

        fn load(&self) -> Result<Vec<Record>, SourceError> {
            if !self.sessions_dir.exists() {
                return Err(SourceError::Absent(self.sessions_dir.to_string_lossy().to_string()));
            }
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
            let mut records = Vec::new();
            let mut files = Vec::new();
            collect_jsonl(&self.sessions_dir, &mut files);
            for p in files {
                let content = match std::fs::read_to_string(&p) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("warn: cannot read {:?}: {}", p, e);
                        continue;
                    }
                };
                let mut project_dir = p.parent();
                let decoded_project = project_dir
                    .and_then(|d| d.file_name())
                    .and_then(|n| n.to_str())
                    .map(|n| Self::decode_dir_name(n, &home));
                let mut session_id: Option<String> = None;
                let mut session_cwd: Option<String> = None;
                let mut session_started_at: Option<DateTime<Utc>> = None;
                let mut events: Vec<(DateTime<Utc>, String, TokenBreakdown)> = Vec::new();
                let mut cost: f64 = 0.0;
                for line in content.lines().filter(|l| !l.trim().is_empty()) {
                    let line: Line = match serde_json::from_str(line) {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    match line.line_type.as_deref() {
                        Some("session") => {
                            session_id = session_id.or(line.id);
                            session_cwd = session_cwd.or(line.cwd);
                            session_started_at =
                                session_started_at.or_else(|| line.timestamp.as_deref().and_then(parse_timestamp));
                        }
                        Some("message") => {
                            let m = match line.message {
                                Some(m) => m,
                                None => continue,
                            };
                            if m.role.as_deref() != Some("assistant") {
                                continue;
                            }
                            let ts = match line.timestamp.as_deref().and_then(parse_timestamp) {
                                Some(ts) => ts,
                                None => continue,
                            };
                            let model = m.model.clone().unwrap_or_default();
                            let u = m.usage.unwrap_or_default();
                            cost += u.cost.as_ref().and_then(|c| c.total).unwrap_or(0.0);
                            events.push((
                                ts,
                                model,
                                TokenBreakdown {
                                    input: u.input.unwrap_or(0),
                                    output: u.output.unwrap_or(0) + u.reasoning_tokens.unwrap_or(0),
                                    cache_read: u.cache_read.unwrap_or(0),
                                    cache_write: u.cache_write.unwrap_or(0),
                                },
                            ));
                        }
                        _ => {}
                    }
                }
                let project = session_cwd
                    .or_else(|| decoded_project.clone())
                    .unwrap_or_else(|| "/unknown".to_string());
                let cost = if cost == 0.0 { None } else { Some(cost) };
                if let Some(rec) = build_session_record(
                    "omp",
                    session_id.unwrap_or_else(|| {
                        p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()
                    }),
                    project,
                    session_started_at,
                    &mut events,
                    cost,
                ) {
                    records.push(rec);
                }
            }
            Ok(records)
        }
    }
}

// ---------------------------------------------------------------------------
// Kilo Code adapter
// ---------------------------------------------------------------------------
mod kilo {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use chrono::{DateTime, TimeZone, Utc};
    use rusqlite::Connection;

    pub struct KiloSource {
        dbs: Vec<PathBuf>,
    }

    impl KiloSource {
        pub fn new(dbs: Vec<PathBuf>) -> Self {
            Self { dbs }
        }
        fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
            Utc.timestamp_millis_opt(ms).single()
        }
        /// Count `message` rows per session. Message counts are a separate
        /// lookup because the `message` table may be absent from older schemas;
        /// a failure there degrades to a zero count instead of failing the source.
        fn message_counts(conn: &Connection, db_path: &Path) -> std::collections::HashMap<String, u32> {
            let mut counts = std::collections::HashMap::new();
            let mut stmt = match conn.prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id") {
                Ok(stmt) => stmt,
                Err(e) => {
                    eprintln!("warn: cannot count messages in {:?}: {}", db_path, e);
                    return counts;
                }
            };
            let rows = match stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            }) {
                Ok(rows) => rows,
                Err(e) => {
                    eprintln!("warn: cannot count messages in {:?}: {}", db_path, e);
                    return counts;
                }
            };
            for pair in rows.flatten() {
                counts.insert(pair.0, pair.1);
            }
            counts
        }
    }

    impl Source for KiloSource {
        fn name(&self) -> &str { "kilo" }

        fn load(&self) -> Result<Vec<Record>, SourceError> {
            if self.dbs.is_empty() {
                return Ok(Vec::new());
            }
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut records = Vec::new();
            for db_path in &self.dbs {
                if !db_path.exists() {
                    return Err(SourceError::Absent(db_path.to_string_lossy().to_string()));
                }
                let conn = match Connection::open(db_path) {
                    Ok(c) => c,
                    Err(e) => return Err(SourceError::Unreadable(e.to_string())),
                };
                let message_counts = Self::message_counts(&conn, db_path);
                let mut stmt = match conn.prepare(
                    "SELECT id, model, agent, directory, cost,
                            tokens_input, tokens_output, tokens_reasoning,
                            tokens_cache_read, tokens_cache_write,
                            time_created, time_updated
                     FROM session"
                ) {
                    Ok(s) => s,
                    Err(e) => return Err(SourceError::Unreadable(e.to_string())),
                };
                let rows = match stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                    ))
                }) {
                    Ok(r) => r,
                    Err(e) => return Err(SourceError::Unreadable(e.to_string())),
                };
                for row in rows {
                    let (
                        id, model_raw, agent, directory, cost,
                        tokens_input, tokens_output, tokens_reasoning,
                        tokens_cache_read, tokens_cache_write,
                        time_created, time_updated,
                    ) = match row {
                        Ok(r) => r,
                        Err(e) => return Err(SourceError::Unreadable(e.to_string())),
                    };
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let started_at = match Self::ms_to_datetime(time_created) {
                        Some(t) => t,
                        None => continue, // skip rows with invalid timestamps
                    };
                    let message_count = message_counts.get(&id).copied().unwrap_or(0);
                    records.push(Record {
                        session_id: id,
                        source: "kilo".to_string(),
                        project: directory,
                        model: normalize_model(&model_raw),
                        agent,
                        started_at,
                        ended_at: Self::ms_to_datetime(time_updated),
                        tokens: TokenBreakdown {
                            input: tokens_input as u64,
                            output: (tokens_output + tokens_reasoning) as u64,
                            cache_read: tokens_cache_read as u64,
                            cache_write: tokens_cache_write as u64,
                        },
                        message_count,
                        cost: Some(cost),
                    });
                }
            }
            Ok(records)
        }
    }
}

pub use claude::ClaudeSource;
pub use kilo::KiloSource;
pub use omp::OmpSource;
pub use opencode::OpenCodeSource;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::{Record, TokenBreakdown};
    use chrono::Utc;

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
    fn claude_loads_nested_cache_and_reasoning_tokens() {
        use super::claude::ClaudeSource;
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-hunter-projects-modbox");
        std::fs::create_dir_all(&proj).unwrap();
        // Real Claude Code `usage` shape: reasoning under `output_tokens_details`,
        // cache creation nested under `cache_creation`, plus flat cache-read.
        std::fs::write(
            proj.join("session-x.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-08-28T22:50:39.957Z","message":{"model":"auto","usage":{"input_tokens":4451,"output_tokens":350,"cache_creation_input_tokens":0,"cache_read_input_tokens":1200,"output_tokens_details":{"reasoning_tokens":80},"cache_creation":{"ephemeral_5m_input_tokens":600,"ephemeral_1h_input_tokens":400}}}}
"#,
        )
        .unwrap();
        let src = ClaudeSource::new(tmp.path().to_path_buf());
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1);
        let t = &records[0].tokens;
        assert_eq!(t.input, 4451);
        // reasoning (80) is folded into output (350) on load.
        assert_eq!(t.output, 430);
        assert_eq!(t.cache_read, 1200);
        assert_eq!(
            t.cache_write, 1000,
            "cache_write must sum nested ephemeral_5m + ephemeral_1h"
        );
        assert_eq!(records[0].model, "auto");
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

    #[test]
    fn omp_decode_dir_name_is_home_relative() {
        use super::omp::OmpSource;
        let home = std::path::Path::new("/home/hunter");
        // Round-trippable (hyphen-free) name.
        assert_eq!(
            OmpSource::decode_dir_name("-projects-modbox", home),
            "/home/hunter/projects/modbox"
        );
        // Bare '-' decodes to $HOME itself.
        assert_eq!(OmpSource::decode_dir_name("-", home), "/home/hunter");
        // NOTE (C4): OMP encodes the home-relative path by replacing every '/'
        // with '-', so a hyphen inside a real project name (e.g. `oc-usage`)
        // is indistinguishable from a separator in the directory name alone.
        // Such names are recovered from the session `cwd` (which the adapter
        // prefers), not from the directory name — by design, not a fallback gap.
    }

    #[test]
    fn omp_loads_sessions_from_jsonl() {
        use super::omp::OmpSource;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let proj = root.join("-projects-omp-test");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("2026-08-28T12-00-00-000Z_01aaa.jsonl"),
            r#"{"type":"session","id":"01aaa","timestamp":"2026-08-28T12:00:00.000Z","cwd":"/home/hunter/projects/omp-test"}
{"type":"message","id":"m1","timestamp":"2026-08-28T12:00:10.000Z","message":{"role":"user","content":[]}}
{"type":"message","id":"m2","timestamp":"2026-08-28T12:01:00.000Z","message":{"role":"assistant","provider":"freellm","model":"auto","usage":{"input":100,"output":50,"reasoningTokens":30,"cacheRead":20,"cacheWrite":5,"totalTokens":205,"cost":{"total":0.001}}}}
{"type":"message","id":"m3","timestamp":"2026-08-28T12:02:00.000Z","message":{"role":"assistant","provider":"freellm","model":"sonnet","usage":{"input":300,"output":150,"reasoningTokens":70,"cacheRead":0,"cacheWrite":0,"totalTokens":520,"cost":{"total":0.002}}}}
{"type":"title_change","id":"t1","title":"x"}
"#,
        )
        .unwrap();
        let src = OmpSource::new(root);
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.source, "omp");
        // C3: started_at prefers the session line's timestamp, not the first message.
        assert_eq!(r.started_at.to_rfc3339(), "2026-08-28T12:00:00+00:00");
        assert_eq!(r.ended_at.unwrap().to_rfc3339(), "2026-08-28T12:02:00+00:00");
        assert_eq!(r.session_id, "01aaa");
        assert_eq!(r.project, "/home/hunter/projects/omp-test");
        // C2: raw model value, not a synthetic provider/model label.
        assert_eq!(r.model, "sonnet"); // last assistant message's model wins
        assert_eq!(
            r.tokens,
            TokenBreakdown {
                input: 400,
                output: 300, // 50+30 (m2) + 150+70 (m3), reasoning folded into output
                cache_read: 20,
                cache_write: 5,
            }
        );
        assert_eq!(r.message_count, 2);
        // B2: recorded non-zero cost -> Some; fixture sums to 0.003.
        assert!((r.cost.unwrap() - 0.003).abs() < 1e-9);
    }

    #[test]
    fn omp_falls_back_to_decoded_dir_name_without_session_entry() {
        use super::omp::OmpSource;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let proj = root.join("-projects-nocwd");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("s.jsonl"),
            r#"{"type":"message","timestamp":"2026-08-28T12:00:00.000Z","message":{"role":"assistant","model":"auto","usage":{"input":1,"output":1}}}
"#,
        )
        .unwrap();
        let src = OmpSource::new(root);
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1);
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            records[0].project,
            home.join("projects/nocwd").to_string_lossy()
        );
        // No session entry: id falls back to the filename.
        assert_eq!(records[0].session_id, "s.jsonl");
        assert_eq!(records[0].model, "auto");
    }

    #[test]
    fn omp_scans_nested_subagent_jsonl() {
        use super::omp::OmpSource;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let proj = root.join("-projects-omp-test");
        let session_dir = proj.join("2026-08-28T12-00-00-000Z_01aaa");
        std::fs::create_dir_all(&session_dir).unwrap();
        // A2: subagent session nested one level deeper than the project dir.
        std::fs::write(
            session_dir.join("Subagent.jsonl"),
            r#"{"type":"session","id":"01sub","timestamp":"2026-08-28T12:00:00.000Z","cwd":"/home/hunter/projects/omp-test"}
{"type":"message","timestamp":"2026-08-28T12:01:00.000Z","message":{"role":"assistant","model":"auto","usage":{"input":10,"output":5,"reasoningTokens":3,"cacheRead":1,"cacheWrite":1,"totalTokens":20}}}
"#,
        )
        .unwrap();
        let src = OmpSource::new(root);
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1, "nested subagent jsonl must be scanned");
        assert_eq!(records[0].tokens.input, 10);
        assert_eq!(records[0].tokens.output, 8); // output 5 + reasoning 3
    }

    #[test]
    fn omp_absent_dir_errors() {
        use super::omp::OmpSource;
        let src = OmpSource::new(PathBuf::from("/nonexistent-omp-12345"));
        assert!(matches!(src.load(), Err(SourceError::Absent(_))));
    }

    /// Open a temp file with a minimal Kilo Code schema (`session` + `message`).
    fn open_kilo_db(path: &Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                 id text PRIMARY KEY,
                 project_id text NOT NULL,
                 directory text NOT NULL,
                 cost real DEFAULT 0 NOT NULL,
                 tokens_input integer DEFAULT 0 NOT NULL,
                 tokens_output integer DEFAULT 0 NOT NULL,
                 tokens_reasoning integer DEFAULT 0 NOT NULL,
                 tokens_cache_read integer DEFAULT 0 NOT NULL,
                 tokens_cache_write integer DEFAULT 0 NOT NULL,
                 agent text,
                 model text,
                 time_created integer NOT NULL,
                 time_updated integer NOT NULL);
             CREATE TABLE message (
                 id text PRIMARY KEY,
                 session_id text NOT NULL,
                 time_created integer NOT NULL,
                 data text NOT NULL DEFAULT '');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn kilo_loads_sessions_from_sqlite() {
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("kilo.db");
        let conn = open_kilo_db(&db);
        conn.execute(
            "INSERT INTO session (id, project_id, directory, model, agent, cost,
                    tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write, time_created, time_updated)
             VALUES (?1, 'p1', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                "ses_fix_001",
                "/home/hunter/repos/test",
                // Real Kilo Code stores the model as a JSON envelope.
                r#"{"id":"auto","providerID":"freellm"}"#,
                "code",
                0.25f64,
                1000i64, 500i64, 300i64, 200i64, 50i64,
                1_700_000_000_000i64, 1_700_000_000_100i64,
            ],
        )
        .unwrap();
        for n in 0..3 {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created) VALUES (?1, 'ses_fix_001', ?2)",
                rusqlite::params![format!("m{}", n), 1_700_000_000_000i64 + n],
            )
            .unwrap();
        }
        drop(conn);

        let src = KiloSource::new(vec![db]);
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.source, "kilo");
        assert_eq!(r.session_id, "ses_fix_001");
        assert_eq!(r.project, "/home/hunter/repos/test");
        // The envelope's `id` is the recorded model, not the whole JSON blob.
        assert_eq!(r.model, "auto");
        assert_eq!(r.agent.as_deref(), Some("code"));
        assert_eq!(
            r.tokens,
            TokenBreakdown {
                input: 1000,
                output: 800, // 500 + 300 reasoning, folded in on load
                cache_read: 200,
                cache_write: 50,
            }
        );
        assert_eq!(r.message_count, 3);
        assert!((r.cost.unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(
            r.started_at.to_rfc3339(),
            "2023-11-14T22:13:20+00:00"
        );
        assert_eq!(
            r.ended_at.unwrap().to_rfc3339(),
            "2023-11-14T22:13:20.100+00:00"
        );
    }

    #[test]
    fn kilo_missing_message_table_counts_zero() {
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("kilo.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                 id text PRIMARY KEY, project_id text NOT NULL, directory text NOT NULL,
                 cost real DEFAULT 0 NOT NULL,
                 tokens_input integer DEFAULT 0 NOT NULL, tokens_output integer DEFAULT 0 NOT NULL,
                 tokens_reasoning integer DEFAULT 0 NOT NULL,
                 tokens_cache_read integer DEFAULT 0 NOT NULL,
                 tokens_cache_write integer DEFAULT 0 NOT NULL,
                 agent text, model text,
                 time_created integer NOT NULL, time_updated integer NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, model, time_created, time_updated, tokens_input)
             VALUES ('ses_x', 'p', '/p', 'raw-model', 1700000000000, 1700000000100, 7)",
            [],
        )
        .unwrap();
        drop(conn);

        let src = KiloSource::new(vec![db]);
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1);
        // No `message` table: message_count degrades to zero, model passes through raw.
        assert_eq!(records[0].message_count, 0);
        assert_eq!(records[0].model, "raw-model");
        assert_eq!(records[0].tokens.input, 7);
    }

    #[test]
    fn kilo_dedupes_session_ids_across_dbs() {
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();
        let mk = |name: &str| {
            let p = tmp.path().join(name);
            let c = open_kilo_db(&p);
            c.execute(
                "INSERT INTO session (id, project_id, directory, model, time_created, time_updated)
                 VALUES ('ses_dup', 'p', '/p', 'auto', 1700000000000, 1700000000100)",
                [],
            )
            .unwrap();
            drop(c);
            p
        };
        let src = KiloSource::new(vec![mk("a.db"), mk("b.db")]);
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1, "duplicate session ids across DBs must collapse");
    }

    #[test]
    fn kilo_absent_db_errors() {
        use super::kilo::KiloSource;
        let src = KiloSource::new(vec![PathBuf::from("/nonexistent-kilo-12345/kilo.db")]);
        assert!(matches!(src.load(), Err(SourceError::Absent(_))));
    }

    #[test]
    fn kilo_no_dbs_returns_empty() {
        use super::kilo::KiloSource;
        assert!(KiloSource::new(vec![]).load().unwrap().is_empty());
    }
}

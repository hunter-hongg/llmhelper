use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;

use crate::cache::{CacheStats, FileFingerprint, MessageCache};
use crate::domain::message::Message;
use crate::domain::record::{Record, TokenBreakdown};
use chrono::{DateTime, TimeZone, Utc};

/// The handle a Source holds to the shared message cache.
///
/// One cache per Source (each writes its own index file), behind an `Arc<Mutex>`
/// because the same corpus is read from both the shell paths and the TUI's
/// refresh loop, and a run-scoped statistics total must reflect every read.
pub type SharedMessageCache = Arc<Mutex<MessageCache>>;

/// Open a fresh cache handle for one Source under `dir`.
///
/// Every Source gets its own index file — one Source's cached rows can never be
/// served to another, because the two extract different fields even from a
/// shared schema (OpenCode records a cost, Kilo a message count).
pub fn cache_handle(dir: &Path, name: &str) -> SharedMessageCache {
    Arc::new(Mutex::new(MessageCache::open(dir, name, true)))
}

/// As [`cache_handle`], but the stored index is ignored: for `--refresh-cache`,
/// whose run must re-extract everything regardless of whether deleting the old
/// index file succeeded (spec 0025's "rewrites the index from scratch").
pub fn cache_handle_fresh(dir: &Path, name: &str) -> SharedMessageCache {
    Arc::new(Mutex::new(MessageCache::open(dir, name, false)))
}

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

/// Per-Source status for a message read. Kept separate from `SourceStatus` so
/// the record-reading commands are unaffected by adding a second read path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageStatus {
    pub name: String,
    pub message_count: usize,
    pub error: Option<SourceError>,
}

pub type MessageStatuses = Vec<MessageStatus>;

pub trait Source: Send + Sync {
    fn name(&self) -> &str;
    fn load(&self) -> Result<Vec<Record>, SourceError>;

    /// Load per-message text for this Source. The default returns an empty set
    /// so a Source that stores no message text degrades to an honest empty
    /// result instead of requiring a special case at every call site.
    fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
        Ok(Vec::new())
    }

    /// Persist this Source's message cache index, if it has one. Default is a
    /// no-op, so a Source that never reads messages needs no cache code at all.
    fn flush_cache(&self) {}

    /// This Source's cache statistics, if it has a cache. Default is `None`, so
    /// an uncached Source is absent from the aggregate rather than contributing
    /// a line of zeroes.
    fn cache_stats(&self) -> Option<CacheStats> {
        None
    }
}

pub struct Registry {
    sources: BTreeMap<String, Box<dyn Source>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
        }
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
    /// Load every Source's messages concurrently, in parallel across Sources.
    ///
    /// The Sources are independent — separate files, separate databases, and (in
    /// cached runs) separate cache indices — so their reads cannot interfere.
    /// Each runs on its own thread, which matters because a large corpus is
    /// dominated by I/O wait and SQLite scans that a single thread serialises.
    ///
    /// The **results** are ordered deterministically: Sources are sorted by name
    /// before the threads spawn and the outputs are reassembled in that order,
    /// so the concatenated corpus — and therefore `search`'s hit ranking — is
    /// identical to the sequential version regardless of which thread finishes
    /// first. (Asserted in `tests/cache_integration.rs`.)
    pub fn load_messages_all(&self) -> (Vec<Message>, MessageStatuses) {
        let mut named: Vec<(&String, &Box<dyn Source>)> = self.sources.iter().collect();
        named.sort_by_key(|(name, _)| name.as_str());

        // One scoped thread per Source; a Source that panics yields an error
        // status rather than aborting the run, matching the tolerance the
        // sequential version has for a single Source's failure.
        let results: Vec<(&String, Result<Vec<Message>, SourceError>)> =
            std::thread::scope(|scope| {
                let handles: Vec<_> = named
                    .iter()
                    .map(|(name, src)| (name, scope.spawn(move || src.load_messages())))
                    .collect();
                handles
                    .into_iter()
                    .map(|(name, handle)| {
                        let result = handle.join().unwrap_or_else(|_| {
                            Err(SourceError::Unreadable(format!(
                                "source {} panicked while loading messages",
                                name
                            )))
                        });
                        (*name, result)
                    })
                    .collect()
            });

        let mut messages = Vec::new();
        let mut statuses = Vec::new();
        for (name, result) in results {
            match result {
                Ok(batch) => {
                    let count = batch.len();
                    messages.extend(batch);
                    statuses.push(MessageStatus {
                        name: name.clone(),
                        message_count: count,
                        error: None,
                    });
                }
                Err(e) => {
                    statuses.push(MessageStatus {
                        name: name.clone(),
                        message_count: 0,
                        error: Some(e),
                    });
                }
            }
        }
        (messages, statuses)
    }
    pub fn source_names(&self) -> Vec<&str> {
        self.sources.keys().map(|s| s.as_str()).collect()
    }

    /// Persist every Source's message cache index.
    ///
    /// Called once at the end of a run that read messages. Sources built without
    /// a cache (or handed a disabled one) have nothing to flush, so their own
    /// `flush` is skipped by the Sources themselves — this walks the registry
    /// and asks each one, rather than `main` tracking which Sources were cached.
    pub fn flush_caches(&self) {
        for src in self.sources.values() {
            src.flush_cache();
        }
    }

    /// Aggregate cache statistics across Sources, or `None` when no Source had
    /// a cache (the `--no-cache` path, or `--messages` absent).
    ///
    /// Summed over Sources because the figure a user acts on is "how much of
    /// this run was served from the cache", which is a whole-corpus question,
    /// not a per-Source one. The per-Source breakdown, if ever needed, is a
    /// separate method rather than a wider return type here.
    pub fn cache_stats(&self) -> Option<CacheStats> {
        let mut total = CacheStats::default();
        let mut any = false;
        for src in self.sources.values() {
            if let Some(s) = src.cache_stats() {
                any = true;
                // The per-Source counts are summed, but `sources` is counted
                // here: only the registry knows how many Sources contributed.
                total.sources += 1;
                total.files_seen += s.files_seen;
                total.files_reused += s.files_reused;
                total.files_extracted += s.files_extracted;
                total.messages_reused += s.messages_reused;
                total.messages_extracted += s.messages_extracted;
            }
        }
        any.then_some(total)
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

/// Convert millisecond epoch to UTC. Shared by the SQLite sources.
fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

/// Extract searchable text from a JSONL `message.content` value.
///
/// Both JSONL sources use the same block vocabulary: a plain string, or a list
/// of blocks keyed by `type`. `text` blocks carry `.text`; `thinking` blocks
/// carry `.thinking` and are tagged with the `thinking` role so the corpus keeps
/// model-internal text separate from conversation without a source-specific
/// field. `tool_use`, `toolCall`, `tool_result`, and `image` blocks hold tool
/// plumbing or binary data and are excluded from the corpus.
fn content_texts(
    content: Option<&serde_json::Value>,
    fallback_role: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(v) = content else {
        return out;
    };
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if !t.is_empty() {
            out.push((fallback_role.to_string(), t.to_string()));
        }
        return out;
    }
    if let Some(arr) = v.as_array() {
        for b in arr {
            let bt = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let raw = match bt {
                "text" => b.get("text").and_then(|t| t.as_str()),
                "thinking" => b.get("thinking").and_then(|t| t.as_str()),
                _ => None,
            };
            if let Some(t) = raw {
                let t = t.trim();
                if !t.is_empty() {
                    let role = if bt == "thinking" {
                        "thinking"
                    } else {
                        fallback_role
                    };
                    out.push((role.to_string(), t.to_string()));
                }
            }
        }
    }
    out
}

/// Merge per-DB message batches, keeping only the first database that contains
/// a given session id — mirroring the dedup `load` applies to records. A
/// session claimed by an earlier database is dropped wholesale from later ones,
/// so a session split across databases cannot yield duplicate messages.
fn merge_message_batches(
    seen: &mut std::collections::HashSet<String>,
    batch: Vec<Message>,
) -> Vec<Message> {
    // Session ids newly claimed by this batch. Every message belongs to its
    // session, so a claimed session keeps all of its messages and a session
    // already claimed elsewhere keeps none.
    let claimed: std::collections::HashSet<String> = batch
        .iter()
        .filter(|m| seen.insert(m.session_id.clone()))
        .map(|m| m.session_id.clone())
        .collect();
    batch
        .into_iter()
        .filter(|m| claimed.contains(&m.session_id))
        .collect()
}

/// Read searchable messages from every database of a SQLite Source, skipping
/// absent paths and deduping sessions across databases.
fn load_sqlite_messages_all(
    dbs: &[PathBuf],
    source: &str,
    cache: Option<&SharedMessageCache>,
) -> Vec<Message> {
    if dbs.is_empty() {
        return Vec::new();
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut messages = Vec::new();
    for db_path in dbs {
        // A configured path that does not exist is skipped rather than failing
        // the Source, matching `load`'s tolerance for the variant databases.
        if !db_path.exists() {
            continue;
        }
        // WAL commits need not touch the main DB fingerprint. Only a known
        // absent WAL permits reuse: errors also fail open to a fresh SQLite
        // read. Resolve symlinks because SQLite puts its WAL beside the real DB.
        let wal_absent = std::fs::canonicalize(db_path).ok().is_some_and(|path| {
            let mut wal = path.into_os_string();
            wal.push("-wal");
            matches!(Path::new(&wal).try_exists(), Ok(false))
        });
        let result = if wal_absent {
            cached_file_messages(cache, db_path, || {
                load_sqlite_messages(db_path, source)
            })
        } else if let Some(handle) = cache {
            // No fingerprint means neither old entries nor WAL-derived results
            // are reused/stored, but extraction is still counted. Once SQLite
            // removes the WAL, checkpoint writes invalidate any old main-file
            // fingerprint; an empty WAL needs no such invalidation.
            // The DB file itself was still reached (its WAL merely disallows
            // reuse), so record it: pruning must not drop an entry for a file
            // this load did visit.
            let seen = std::fs::canonicalize(db_path)
                .unwrap_or_else(|_| db_path.to_path_buf());
            handle.lock().mark_seen(&seen);
            handle
                .lock()
                .reuse_or_extract(None, || load_sqlite_messages(db_path, source))
        } else {
            load_sqlite_messages(db_path, source)
        };
        // Cross-DB session dedup depends on every DB and is never cached.
        let batch = match result {
            Ok(b) => b,
            Err(e) => {
                eprintln!("warn: cannot read messages from {:?}: {}", db_path, e);
                continue;
            }
        };
        messages.extend(merge_message_batches(&mut seen, batch));
    }
    messages
}

/// Read searchable messages from the `part` table of one SQLite database.
///
/// OpenCode and Kilo Code share an identical `session`/`message`/`part` schema,
/// so a single query serves both. Each `part.data` row is a JSON object whose
/// `type` selects its shape: `text` and `reasoning` both carry `.text` and are
/// the only part types with readable prose. Parts flagged `synthetic` or
/// `ignored` are excluded, matching the filter Kilo Code applies in its own
/// `recall_part_search_idx`.
fn load_sqlite_messages(db_path: &Path, source: &str) -> Result<Vec<Message>, SourceError> {
    if !db_path.exists() {
        return Err(SourceError::Absent(db_path.to_string_lossy().to_string()));
    }
    let conn = match Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => return Err(SourceError::Unreadable(e.to_string())),
    };
    let mut stmt = match conn.prepare(
        "SELECT m.session_id, m.data, p.data, p.time_created, s.directory, s.model
         FROM part p
         JOIN message m ON m.id = p.message_id
         JOIN session s ON s.id = m.session_id
         WHERE json_valid(p.data)
           AND json_extract(p.data, '$.type') IN ('text', 'reasoning')
           AND coalesce(json_extract(p.data, '$.synthetic'), 0) = 0
           AND coalesce(json_extract(p.data, '$.ignored'), 0) = 0
         ORDER BY p.time_created, p.id",
    ) {
        Ok(s) => s,
        // A missing `part` or `message` table is an older schema, not a broken
        // database. The Source still reports records, so this degrades to an
        // empty message set rather than failing the whole Source.
        Err(_) => return Ok(Vec::new()),
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            // `model` is nullable in the real schema; binding NULL to String
            // would drop the row silently.
            row.get::<_, Option<String>>(5)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(e) => return Err(SourceError::Unreadable(e.to_string())),
    };
    let mut messages = Vec::new();
    for row in rows.flatten() {
        let (session_id, msg_data, part_data, time_created, directory, model_raw) = row;
        let Some(part) = serde_json::from_str::<serde_json::Value>(&part_data).ok() else {
            continue;
        };
        let is_reasoning = part.get("type").and_then(|t| t.as_str()) == Some("reasoning");
        let Some(text) = part
            .get("text")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        // Reasoning is model-internal text; tag it so it stays separable from
        // conversation without a source-specific field.
        let role = if is_reasoning {
            "thinking".to_string()
        } else {
            serde_json::from_str::<serde_json::Value>(&msg_data)
                .ok()
                .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(str::to_string))
                .unwrap_or_else(|| "assistant".to_string())
        };
        let model = model_raw.as_deref().map(normalize_model);
        let model = model.filter(|m| !m.is_empty());
        messages.push(Message {
            source: source.to_string(),
            session_id,
            project: directory,
            model,
            role,
            timestamp: ms_to_datetime(time_created),
            text: text.to_string(),
        });
    }
    Ok(messages)
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

/// Read one file's messages, consulting the cache first.
///
/// A `None` cache (caching off, or a Source constructed without one) and an
/// uncapturable fingerprint both degrade to running `extract`, so a run without
/// a cache behaves exactly as it did before — only slower.
fn cached_file_messages<F>(
    cache: Option<&SharedMessageCache>,
    path: &Path,
    extract: F,
) -> Result<Vec<Message>, SourceError>
where
    F: FnOnce() -> Result<Vec<Message>, SourceError>,
{
    match cache {
        Some(handle) => {
            let fingerprint = FileFingerprint::capture(path);
            handle
                .lock()
                .reuse_or_extract(fingerprint.as_ref(), extract)
        }
        None => extract(),
    }
}

/// One OMP message collected before the session envelope is resolved:
/// `(timestamp, model, role, text)`.
type MessageEvent = (Option<DateTime<Utc>>, Option<String>, String, String);

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
        content: Option<serde_json::Value>,
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
        cache: Option<SharedMessageCache>,
    }

    impl ClaudeSource {
        pub fn new(project_dir: PathBuf) -> Self {
            Self {
                project_dir,
                cache: None,
            }
        }

        /// As [`Self::new`], with a message cache attached for `load_messages`.
        pub fn with_cache(project_dir: PathBuf, cache: SharedMessageCache) -> Self {
            Self {
                project_dir,
                cache: Some(cache),
            }
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
        fn name(&self) -> &str {
            "claude"
        }

        fn load(&self) -> Result<Vec<Record>, SourceError> {
            if !self.project_dir.exists() {
                return Err(SourceError::Absent(
                    self.project_dir.to_string_lossy().to_string(),
                ));
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
                            .or(u.cache_creation_input_tokens)
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

        fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
            // The handle (and its stats) survive the TUI's `r` refresh; the
            // load must describe this walk only — zeroed counters, and a
            // corpus walk announced so stale index entries are pruned at flush
            // (spec 0025).
            if let Some(c) = self.cache.as_ref() {
                c.lock().begin_load();
            }
            if !self.project_dir.exists() {
                return Err(SourceError::Absent(
                    self.project_dir.to_string_lossy().to_string(),
                ));
            }
            let mut messages = Vec::new();
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
                    let batch = cached_file_messages(self.cache.as_ref(), &p, || {
                        Self::extract_file_messages(&p, &project)
                    })?;
                    messages.extend(batch);
                }
            }
            Ok(messages)
        }

        fn flush_cache(&self) {
            if let Some(c) = self.cache.as_ref() {
                c.lock().flush();
            }
        }

        fn cache_stats(&self) -> Option<CacheStats> {
            self.cache.as_ref().map(|c| c.lock().stats())
        }
    }

    impl ClaudeSource {
        /// Extract every message in one transcript file.
        ///
        /// The unit the cache memoizes: a Claude transcript file is one session,
        /// and its `session_id` is the file name and its `project` the enclosing
        /// directory, so the extraction is fully determined by the file's
        /// content — exactly what makes it safe to reuse a cached result.
        fn extract_file_messages(p: &Path, project: &str) -> Result<Vec<Message>, SourceError> {
            let content = match std::fs::read_to_string(p) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("warn: cannot read {:?}: {}", p, e);
                    return Ok(Vec::new());
                }
            };
            let session_id = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let mut messages = Vec::new();
            for line in content.lines().filter(|l| !l.trim().is_empty()) {
                let line: Line = match serde_json::from_str(line) {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                let Some(m) = line.message.as_ref() else {
                    continue;
                };
                // `user` and `assistant` are the conversation turns.
                // Every other line type is harness bookkeeping.
                let role = match line.line_type.as_deref() {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    _ => continue,
                };
                let ts = line.timestamp.as_deref().and_then(parse_timestamp);
                let model = m.model.clone();
                for (msg_role, text) in content_texts(m.content.as_ref(), role) {
                    messages.push(Message {
                        source: "claude".to_string(),
                        session_id: session_id.clone(),
                        project: project.to_string(),
                        model: model.clone(),
                        role: msg_role,
                        timestamp: ts,
                        text,
                    });
                }
            }
            Ok(messages)
        }
    }
}

// ---------------------------------------------------------------------------
// OpenCode adapter
// ---------------------------------------------------------------------------

mod opencode {
    use super::*;
    use crate::domain::record::{Record, TokenBreakdown};
    use rusqlite::Connection;

    pub struct OpenCodeSource {
        dbs: Vec<PathBuf>,
        cache: Option<SharedMessageCache>,
    }

    impl OpenCodeSource {
        pub fn new(dbs: Vec<PathBuf>) -> Self {
            Self { dbs, cache: None }
        }

        /// As [`Self::new`], with a message cache attached for `load_messages`.
        pub fn with_cache(dbs: Vec<PathBuf>, cache: SharedMessageCache) -> Self {
            Self {
                dbs,
                cache: Some(cache),
            }
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
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
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
                     FROM session",
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
                        // `model` is nullable in the real schema; a NULL model
                        // must not drop the whole record.
                        row.get::<_, Option<String>>(1)?,
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
                        id,
                        model_raw,
                        agent,
                        directory,
                        _title,
                        cost,
                        tokens_input,
                        tokens_output,
                        tokens_reasoning,
                        tokens_cache_read,
                        tokens_cache_write,
                        time_created,
                        time_updated,
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
                    let started_at = match ms_to_datetime(time_created) {
                        Some(t) => t,
                        None => continue, // skip rows with invalid timestamps
                    };
                    let ended_at = ms_to_datetime(time_updated);
                    records.push(Record {
                        session_id: id,
                        source: "opencode".to_string(),
                        project: directory,
                        model: model_raw
                            .as_deref()
                            .map(normalize_model)
                            .unwrap_or_default(),
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

        fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
            // The handle outlives the run (the TUI's `r` refresh reuses it),
            // so re-open the load state: zeroed counters and a fresh corpus
            // walk, so stale index entries are pruned at flush (spec 0025).
            if let Some(c) = self.cache.as_ref() {
                c.lock().begin_load();
            }
            Ok(load_sqlite_messages_all(
                &self.dbs,
                "opencode",
                self.cache.as_ref(),
            ))
        }

        fn flush_cache(&self) {
            if let Some(c) = self.cache.as_ref() {
                c.lock().flush();
            }
        }

        fn cache_stats(&self) -> Option<CacheStats> {
            self.cache.as_ref().map(|c| c.lock().stats())
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
        /// Populated on `type == "custom_message"` lines, which carry their
        /// text at the top level rather than under `message`.
        #[serde(rename = "customType")]
        custom_type: Option<String>,
        #[serde(default)]
        content: Option<serde_json::Value>,
        #[serde(default)]
        message: Option<MessageInner>,
    }

    #[derive(serde::Deserialize, Debug)]
    struct MessageInner {
        role: Option<String>,
        model: Option<String>,
        #[allow(dead_code)]
        provider: Option<String>,
        usage: Option<Usage>,
        content: Option<serde_json::Value>,
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
        cache: Option<SharedMessageCache>,
    }

    /// One OMP file's two halves: its aggregate record (when it has timestamped
    /// assistant turns) and its message corpus.
    struct OmpFileExtract {
        record: Option<Record>,
        messages: Vec<Message>,
    }

    impl OmpSource {
        pub fn new(sessions_dir: PathBuf) -> Self {
            Self {
                sessions_dir,
                cache: None,
            }
        }

        /// As [`Self::new`], with a message cache attached. Only the message
        /// half of a warmed file is reused; the record half is always computed
        /// from disk, so record-reading commands see no behavioural change.
        pub fn with_cache(sessions_dir: PathBuf, cache: SharedMessageCache) -> Self {
            Self {
                sessions_dir,
                cache: Some(cache),
            }
        }

        /// Walk every session JSONL once, producing both the aggregated
        /// records and the raw message corpus. Without this the two reads each
        /// walked the whole directory, which matters on a machine where the
        /// transcripts run to hundreds of megabytes.
        ///
        /// The caller decides which half to use, so one warm-cache extract can
        /// legitimately return no records (see [`Self::extract_file`]).
        fn load_all(&self) -> Result<(Vec<Record>, Vec<Message>), SourceError> {
            if !self.sessions_dir.exists() {
                return Err(SourceError::Absent(
                    self.sessions_dir.to_string_lossy().to_string(),
                ));
            }
            let mut records = Vec::new();
            let mut messages = Vec::new();
            let mut files = Vec::new();
            collect_jsonl(&self.sessions_dir, &mut files);
            for p in files {
                // A cache hit serves the message half only; the record half
                // still comes from the file. Letting the record half be skipped
                // too would couple `usage`'s speed to the cache for no gain.
                let extracted = self.extract_file(&p)?;
                if let Some(rec) = extracted.record {
                    records.push(rec);
                }
                messages.extend(extracted.messages);
            }
            Ok((records, messages))
        }

        /// Extract one file, consulting the cache for the message half.
        ///
        /// Split deliberately in two: the cache is consulted only for a file's
        /// messages, and on a miss the **same** parse pass that produces the
        /// messages also produces the record, so a cold cache costs no extra
        /// read. The record is never reused — `usage` sees unchanged numbers —
        /// so a caller that needs records must not go through this path with a
        /// warm cache (see [`Self::load_all`], and `Source::load`'s fresh
        /// un-cached walk for message-cached Sources).
        fn extract_file(&self, p: &Path) -> Result<OmpFileExtract, SourceError> {
            let fingerprint = FileFingerprint::capture(p);
            if let (Some(handle), Some(fp)) = (self.cache.as_ref(), fingerprint.as_ref()) {
                handle.lock().mark_seen(&fp.path);
                if let Some(messages) = handle.lock().lookup(fp) {
                    return Ok(OmpFileExtract {
                        record: None,
                        messages,
                    });
                }
            }
            let extracted = Self::extract_file_uncached(p)?;
            if let (Some(handle), Some(fp)) = (self.cache.as_ref(), fingerprint.as_ref()) {
                handle.lock().record(fp, extracted.messages.clone());
            }
            Ok(extracted)
        }

        /// Parse one OMP session file into its record and messages. The unit the
        /// cache memoizes is this whole extraction: a session envelope is spread
        /// across lines that precede the messages referencing it, so a per-line
        /// cache would lose the `cwd`/`id` that only appear at the top.
        fn extract_file_uncached(p: &Path) -> Result<OmpFileExtract, SourceError> {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
            let content = match std::fs::read_to_string(p) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("warn: cannot read {:?}: {}", p, e);
                    return Ok(OmpFileExtract {
                        record: None,
                        messages: Vec::new(),
                    });
                }
            };
            let decoded_project = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .map(|n| Self::decode_dir_name(n, &home));
            let mut session_id: Option<String> = None;
            let mut session_cwd: Option<String> = None;
            let mut session_started_at: Option<DateTime<Utc>> = None;
            let mut events: Vec<(DateTime<Utc>, String, TokenBreakdown)> = Vec::new();
            let mut cost: f64 = 0.0;
            // (timestamp, model, role, text) — resolved against the session
            // envelope after the loop, matching how `load` builds a record.
            let mut message_events: Vec<MessageEvent> = Vec::new();
            for line in content.lines().filter(|l| !l.trim().is_empty()) {
                let line: Line = match serde_json::from_str(line) {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                let ts = line.timestamp.as_deref().and_then(parse_timestamp);
                match line.line_type.as_deref() {
                    Some("session") => {
                        session_id = session_id.or(line.id);
                        session_cwd = session_cwd.or(line.cwd);
                        session_started_at = session_started_at.or(ts);
                    }
                    Some("message") => {
                        let Some(m) = line.message else {
                            continue;
                        };
                        // Record side: only assistant turns with a timestamp
                        // contribute usage.
                        if m.role.as_deref() == Some("assistant") {
                            if let Some(ts) = ts {
                                let model = m.model.clone().unwrap_or_default();
                                let u = m.usage.unwrap_or_default();
                                cost += u.cost.as_ref().and_then(|c| c.total).unwrap_or(0.0);
                                events.push((
                                    ts,
                                    model,
                                    TokenBreakdown {
                                        input: u.input.unwrap_or(0),
                                        output: u.output.unwrap_or(0)
                                            + u.reasoning_tokens.unwrap_or(0),
                                        cache_read: u.cache_read.unwrap_or(0),
                                        cache_write: u.cache_write.unwrap_or(0),
                                    },
                                ));
                            }
                        }
                        // Corpus side: tool and shell output are machine
                        // dumps, not conversation, and dominate the corpus
                        // by volume.
                        match m.role.as_deref() {
                            Some("toolResult") | Some("bashExecution") => continue,
                            _ => {}
                        }
                        let Some(role) = m.role.clone() else {
                            continue;
                        };
                        let model = m.model.clone();
                        for (msg_role, text) in content_texts(m.content.as_ref(), &role) {
                            message_events.push((ts, model.clone(), msg_role, text));
                        }
                    }
                    Some("custom_message") => {
                        let Some(v) = line.content.as_ref() else {
                            continue;
                        };
                        let role = line
                            .custom_type
                            .clone()
                            .unwrap_or_else(|| "custom".to_string());
                        for (msg_role, text) in content_texts(Some(v), &role) {
                            message_events.push((ts, None, msg_role, text));
                        }
                    }
                    _ => {}
                }
            }
            let project = session_cwd
                .or_else(|| decoded_project.clone())
                .unwrap_or_else(|| "/unknown".to_string());
            let sid = session_id.unwrap_or_else(|| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string()
            });
            let cost = if cost == 0.0 { None } else { Some(cost) };
            let record = build_session_record(
                "omp",
                sid.clone(),
                project.clone(),
                session_started_at,
                &mut events,
                cost,
            );
            let messages = message_events
                .into_iter()
                .map(|(ts, model, role, text)| Message {
                    source: "omp".to_string(),
                    session_id: sid.clone(),
                    project: project.clone(),
                    model,
                    role,
                    timestamp: ts,
                    text,
                })
                .collect();
            Ok(OmpFileExtract { record, messages })
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
        fn name(&self) -> &str {
            "omp"
        }

        fn load(&self) -> Result<Vec<Record>, SourceError> {
            // Message-cache hits contain no record; record reads must always
            // parse the source, even when this Source has a warmed cache.
            let (records, _) = Self::new(self.sessions_dir.clone()).load_all()?;
            Ok(records)
        }

        fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
            // The handle outlives the run (the TUI's `r` refresh reuses it),
            // so re-open the load state: zeroed counters and a fresh corpus
            // walk, so stale index entries are pruned at flush (spec 0025).
            if let Some(c) = self.cache.as_ref() {
                c.lock().begin_load();
            }
            let (_, messages) = self.load_all()?;
            Ok(messages)
        }

        fn flush_cache(&self) {
            if let Some(c) = self.cache.as_ref() {
                c.lock().flush();
            }
        }

        fn cache_stats(&self) -> Option<CacheStats> {
            self.cache.as_ref().map(|c| c.lock().stats())
        }
    }
}

// ---------------------------------------------------------------------------
// Kilo Code adapter
// ---------------------------------------------------------------------------
mod kilo {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use rusqlite::Connection;

    pub struct KiloSource {
        dbs: Vec<PathBuf>,
        cache: Option<SharedMessageCache>,
    }

    impl KiloSource {
        pub fn new(dbs: Vec<PathBuf>) -> Self {
            Self { dbs, cache: None }
        }

        /// As [`Self::new`], with a message cache attached for `load_messages`.
        pub fn with_cache(dbs: Vec<PathBuf>, cache: SharedMessageCache) -> Self {
            Self {
                dbs,
                cache: Some(cache),
            }
        }
        /// Count `message` rows per session. Message counts are a separate
        /// lookup because the `message` table may be absent from older schemas;
        /// a failure there degrades to a zero count instead of failing the source.
        fn message_counts(
            conn: &Connection,
            db_path: &Path,
        ) -> std::collections::HashMap<String, u32> {
            let mut counts = std::collections::HashMap::new();
            let mut stmt = match conn
                .prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id")
            {
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
        fn name(&self) -> &str {
            "kilo"
        }

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
                     FROM session",
                ) {
                    Ok(s) => s,
                    Err(e) => return Err(SourceError::Unreadable(e.to_string())),
                };
                let rows = match stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        // `model` is nullable in the real schema; a NULL model
                        // must not drop the whole record.
                        row.get::<_, Option<String>>(1)?,
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
                        id,
                        model_raw,
                        agent,
                        directory,
                        cost,
                        tokens_input,
                        tokens_output,
                        tokens_reasoning,
                        tokens_cache_read,
                        tokens_cache_write,
                        time_created,
                        time_updated,
                    ) = match row {
                        Ok(r) => r,
                        Err(e) => return Err(SourceError::Unreadable(e.to_string())),
                    };
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let started_at = match ms_to_datetime(time_created) {
                        Some(t) => t,
                        None => continue, // skip rows with invalid timestamps
                    };
                    let message_count = message_counts.get(&id).copied().unwrap_or(0);
                    records.push(Record {
                        session_id: id,
                        source: "kilo".to_string(),
                        project: directory,
                        model: model_raw
                            .as_deref()
                            .map(normalize_model)
                            .unwrap_or_default(),
                        agent,
                        started_at,
                        ended_at: ms_to_datetime(time_updated),
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

        fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
            // The handle outlives the run (the TUI's `r` refresh reuses it),
            // so re-open the load state: zeroed counters and a fresh corpus
            // walk, so stale index entries are pruned at flush (spec 0025).
            if let Some(c) = self.cache.as_ref() {
                c.lock().begin_load();
            }
            Ok(load_sqlite_messages_all(
                &self.dbs,
                "kilo",
                self.cache.as_ref(),
            ))
        }

        fn flush_cache(&self) {
            if let Some(c) = self.cache.as_ref() {
                c.lock().flush();
            }
        }

        fn cache_stats(&self) -> Option<CacheStats> {
            self.cache.as_ref().map(|c| c.lock().stats())
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

    #[test]
    fn load_messages_all_is_ordered_by_source_name() {
        // Two Sources whose load is deliberately slow-then-fast in *reverse*
        // name order: "aaa" sleeps and "zzz" returns immediately. If the
        // registry concatenated results in completion order, "zzz" would come
        // first; ordering by name must keep "aaa" first.
        struct SlowSource {
            name: &'static str,
            sleep_ms: u64,
        }
        impl Source for SlowSource {
            fn name(&self) -> &str {
                self.name
            }
            fn load(&self) -> Result<Vec<Record>, SourceError> {
                Ok(Vec::new())
            }
            fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
                std::thread::sleep(std::time::Duration::from_millis(self.sleep_ms));
                Ok(vec![Message {
                    source: self.name.to_string(),
                    session_id: "s".to_string(),
                    project: String::new(),
                    model: None,
                    role: "user".to_string(),
                    timestamp: None,
                    text: self.name.to_string(),
                }])
            }
        }
        let mut reg = Registry::new();
        reg.register(Box::new(SlowSource {
            name: "aaa",
            sleep_ms: 50,
        }));
        reg.register(Box::new(SlowSource {
            name: "zzz",
            sleep_ms: 0,
        }));

        let (messages, statuses) = reg.load_messages_all();
        assert_eq!(
            messages
                .iter()
                .map(|m| m.source.as_str())
                .collect::<Vec<_>>(),
            vec!["aaa", "zzz"],
            "sources must be concatenated in name order, not completion order"
        );
        assert_eq!(statuses[0].name, "aaa");
        assert_eq!(statuses[1].name, "zzz");
    }

    #[test]
    fn a_panicking_source_is_reported_not_aborted() {
        struct PanicSource;
        impl Source for PanicSource {
            fn name(&self) -> &str {
                "boom"
            }
            fn load(&self) -> Result<Vec<Record>, SourceError> {
                Ok(Vec::new())
            }
            fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
                panic!("synthetic source failure");
            }
        }
        let mut reg = Registry::new();
        reg.register(Box::new(PanicSource));
        let (messages, statuses) = reg.load_messages_all();
        assert!(messages.is_empty());
        assert_eq!(statuses.len(), 1);
        assert!(
            statuses[0].error.is_some(),
            "a panicking source must surface as an error status"
        );
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
        assert_eq!(ClaudeSource::decode_project_name("-single"), "/single");
        assert_eq!(ClaudeSource::decode_project_name("no-prefix"), "no/prefix");
    }

    #[test]
    fn claude_cache_reuses_messages_on_second_load() {
        use super::cache_handle;
        use super::claude::ClaudeSource;
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-hunter-projects-cache");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("session-a.jsonl"),
            r#"{"type":"user","timestamp":"2026-08-28T12:00:00.000Z","message":{"role":"user","content":"cached message"}}"#,
        )
        .unwrap();

        let cache_dir = tmp.path().join("cache");
        let cache = cache_handle(&cache_dir, "claude");
        let src = ClaudeSource::with_cache(tmp.path().to_path_buf(), cache);
        let first = src.load_messages().unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, "cached message");
        src.flush_cache();

        // A second run is a fresh handle over the persisted index.
        let warm = cache_handle(&cache_dir, "claude");
        let src2 = ClaudeSource::with_cache(tmp.path().to_path_buf(), warm.clone());
        assert_eq!(src2.load_messages().unwrap(), first);

        let stats = warm.lock().stats();
        assert!(stats.is_observed());
        assert_eq!(stats.files_seen, 1);
        assert_eq!(stats.files_reused, 1);
        assert_eq!(stats.files_extracted, 0);
        assert_eq!(stats.messages_reused, 1);
        assert_eq!(stats.messages_extracted, 0);
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
        assert_eq!(
            r.ended_at.unwrap().to_rfc3339(),
            "2026-08-28T12:02:00+00:00"
        );
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
                1000i64,
                500i64,
                300i64,
                200i64,
                50i64,
                1_700_000_000_000i64,
                1_700_000_000_100i64,
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
        assert_eq!(r.started_at.to_rfc3339(), "2023-11-14T22:13:20+00:00");
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
        assert_eq!(
            records.len(),
            1,
            "duplicate session ids across DBs must collapse"
        );
    }

    #[test]
    fn kilo_null_model_does_not_drop_the_record() {
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("kilo.db");
        let conn = open_kilo_db(&db);
        conn.execute(
            "INSERT INTO session (id, project_id, directory, time_created, time_updated, tokens_input)
             VALUES ('ses_nomodel', 'p', '/p', 1700000000000, 1700000000100, 4)",
            [],
        )
        .unwrap();
        drop(conn);

        let src = KiloSource::new(vec![db]);
        let records = src.load().unwrap();
        assert_eq!(
            records.len(),
            1,
            "a NULL model must not silently drop a session"
        );
        assert_eq!(records[0].model, "");
        // And the message read must behave the same way.
        assert!(src.load_messages().unwrap().is_empty());
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

    #[test]
    fn claude_loads_messages_with_roles_and_drops_tool_blocks() {
        use super::claude::ClaudeSource;
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-hunter-projects-test");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("session-x.jsonl"),
            r#"{"type":"user","timestamp":"2026-08-28T12:00:00.000Z","message":{"role":"user","content":"hello there"}}
{"type":"user","timestamp":"2026-08-28T12:00:01.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"FILE DUMP"},{"type":"text","text":"and more"}]}}
{"type":"assistant","timestamp":"2026-08-28T12:00:02.000Z","message":{"model":"auto","content":[{"type":"thinking","thinking":"inner monologue"},{"type":"tool_use","id":"t2","name":"Read","input":{}},{"type":"text","text":"the answer is forty two"}]}}
{"type":"system","timestamp":"2026-08-28T12:00:03.000Z","message":{"role":"system","content":[{"type":"text","text":"harness bookkeeping line"}]}}
"#,
        )
        .unwrap();
        let src = ClaudeSource::new(tmp.path().to_path_buf());
        let messages = src.load_messages().unwrap();
        let got: Vec<(&str, &str)> = messages
            .iter()
            .map(|m| (m.role.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("user", "hello there"),
                ("user", "and more"),
                ("thinking", "inner monologue"),
                ("assistant", "the answer is forty two"),
            ]
        );
        assert!(messages.iter().all(|m| m.session_id == "session-x.jsonl"));
        assert!(messages
            .iter()
            .all(|m| m.project == "/home/hunter/projects/test"));
        assert!(messages.iter().all(|m| m.source == "claude"));
        // Model comes from the assistant line that carried it; user lines have none.
        assert_eq!(messages[0].model, None);
        assert_eq!(messages[2].model.as_deref(), Some("auto"));
        assert!(messages.iter().all(|m| m.timestamp.is_some()));
    }

    #[test]
    fn omp_loads_messages_and_drops_tool_output() {
        use super::omp::OmpSource;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let proj = root.join("-projects-omp-test");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("2026-08-28T12-00-00-000Z_01aaa.jsonl"),
            r#"{"type":"session","id":"01aaa","timestamp":"2026-08-28T12:00:00.000Z","cwd":"/home/hunter/projects/omp-test"}
{"type":"message","id":"m0","timestamp":"2026-08-28T12:00:10.000Z","message":{"role":"user","model":"auto","content":[{"type":"text","text":"question one"}]}}
{"type":"message","id":"m1","timestamp":"2026-08-28T12:00:20.000Z","message":{"role":"assistant","model":"auto","content":[{"type":"thinking","thinking":"pondering"},{"type":"toolCall","name":"bash","arguments":{}},{"type":"text","text":"answer one"}]}}
{"type":"message","id":"m2","timestamp":"2026-08-28T12:00:30.000Z","message":{"role":"toolResult","model":"auto","content":[{"type":"text","text":"HIDDEN TOOL OUTPUT"}]}}
{"type":"custom_message","id":"mc1","timestamp":"2026-08-28T12:00:40.000Z","customType":"mid-run-todo-nudge","content":"todo reminder body"}
"#,
        )
        .unwrap();
        let src = OmpSource::new(root);
        let messages = src.load_messages().unwrap();
        let got: Vec<(&str, &str)> = messages
            .iter()
            .map(|m| (m.role.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("user", "question one"),
                ("thinking", "pondering"),
                ("assistant", "answer one"),
                ("mid-run-todo-nudge", "todo reminder body"),
            ]
        );
        assert!(messages.iter().all(|m| m.session_id == "01aaa"));
        assert!(messages
            .iter()
            .all(|m| m.project == "/home/hunter/projects/omp-test"));
        assert!(messages.iter().all(|m| m.source == "omp"));
        assert!(!messages.iter().any(|m| m.text.contains("HIDDEN")));
        assert_eq!(messages[0].model.as_deref(), Some("auto"));
        // custom_message lines carry no model.
        assert_eq!(messages[3].model, None);
    }

    #[test]
    fn omp_cache_reuses_messages_on_second_load() {
        use super::cache_handle;
        use super::omp::OmpSource;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let proj = root.join("-projects-omp-cache");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("2026-08-28T12-00-00-000Z_01aaa.jsonl"),
            r#"{"type":"session","id":"01aaa","timestamp":"2026-08-28T12:00:00.000Z","cwd":"/home/hunter/projects/omp-cache"}
{"type":"message","id":"m0","timestamp":"2026-08-28T12:00:10.000Z","message":{"role":"user","model":"auto","content":[{"type":"text","text":"cached question"}]}}
{"type":"message","id":"m1","timestamp":"2026-08-28T12:00:20.000Z","message":{"role":"assistant","model":"auto","content":[{"type":"text","text":"cached answer"}]}}
"#,
        )
        .unwrap();

        let cache_dir = tmp.path().join("cache");
        let cache = cache_handle(&cache_dir, "omp");
        let src = OmpSource::with_cache(root.clone(), cache);
        let first = src.load_messages().unwrap();
        assert_eq!(first.len(), 2);
        src.flush_cache();

        // A second run is a fresh handle over the persisted index.
        let warm = cache_handle(&cache_dir, "omp");
        let src2 = OmpSource::with_cache(root, warm.clone());
        assert_eq!(src2.load_messages().unwrap(), first);

        let stats = warm.lock().stats();
        assert!(stats.is_observed());
        assert_eq!(stats.files_seen, 1);
        assert_eq!(stats.files_reused, 1);
        assert_eq!(stats.files_extracted, 0);
        assert_eq!(stats.messages_reused, 2);
        assert_eq!(stats.messages_extracted, 0);
    }

    /// Open a temp SQLite file with the shared OpenCode/Kilo Code schema.
    fn open_sqlite_text_db(path: &Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                  id text PRIMARY KEY,
                  project_id text NOT NULL,
                  directory text NOT NULL,
                  model text,
                  cost real DEFAULT 0 NOT NULL,
                  tokens_input integer DEFAULT 0 NOT NULL,
                  tokens_output integer DEFAULT 0 NOT NULL,
                  tokens_reasoning integer DEFAULT 0 NOT NULL,
                  tokens_cache_read integer DEFAULT 0 NOT NULL,
                  tokens_cache_write integer DEFAULT 0 NOT NULL,
                  time_created integer NOT NULL,
                  time_updated integer NOT NULL);
              CREATE TABLE message (
                  id text PRIMARY KEY,
                  session_id text NOT NULL,
                  time_created integer NOT NULL,
                  time_updated integer NOT NULL,
                  data text NOT NULL);
              CREATE TABLE part (
                  id text PRIMARY KEY,
                  message_id text NOT NULL,
                  session_id text NOT NULL,
                  time_created integer NOT NULL,
                  time_updated integer NOT NULL,
                  data text NOT NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn sqlite_sources_load_messages_from_part_rows() {
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("kilo.db");
        let conn = open_sqlite_text_db(&db);
        conn.execute(
            "INSERT INTO session (id, project_id, directory, model, time_created, time_updated)
             VALUES ('ses_x', 'p', '/home/hunter/repos/test', ?1, 1700000000000, 1700000000100)",
            rusqlite::params![r#"{"id":"auto","providerID":"freellm"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('m1', 'ses_x', 1700000000010, 1700000000010, '{\"role\":\"user\"}'),
                    ('m2', 'ses_x', 1700000000020, 1700000000020, '{\"role\":\"assistant\"}'),
                    ('m3', 'ses_x', 1700000000030, 1700000000030, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO part VALUES ('p1','m1','ses_x',1700000000011,1700000000011,'{\"type\":\"text\",\"text\":\"user question text\"}');
             INSERT INTO part VALUES ('p2','m2','ses_x',1700000000021,1700000000021,'{\"type\":\"reasoning\",\"text\":\"model reasoning text\",\"time\":{\"start\":1,\"end\":2}}');
             INSERT INTO part VALUES ('p3','m2','ses_x',1700000000022,1700000000022,'{\"type\":\"text\",\"text\":\"assistant reply text\"}');
             INSERT INTO part VALUES ('p4','m2','ses_x',1700000000023,1700000000023,'{\"type\":\"tool\",\"tool\":\"bash\",\"state\":{\"output\":\"HIDDEN\"}}');
             INSERT INTO part VALUES ('p5','m2','ses_x',1700000000024,1700000000024,'{\"type\":\"text\",\"text\":\"synthetic narration\",\"synthetic\":true}');
             INSERT INTO part VALUES ('p6','m3','ses_x',1700000000031,1700000000031,'{\"type\":\"text\",\"text\":\"ignored content\",\"ignored\":true}');
             INSERT INTO part VALUES ('p7','m3','ses_x',1700000000032,1700000000032,'{\"type\":\"text\",\"text\":\"   \"}');
             INSERT INTO part VALUES ('p8','m3','ses_x',1700000000033,1700000000033,'not json at all');",
        )
        .unwrap();
        drop(conn);

        let src = KiloSource::new(vec![db]);
        let messages = src.load_messages().unwrap();
        let got: Vec<(&str, &str)> = messages
            .iter()
            .map(|m| (m.role.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("user", "user question text"),
                ("thinking", "model reasoning text"),
                ("assistant", "assistant reply text"),
            ]
        );
        assert!(messages.iter().all(|m| m.session_id == "ses_x"));
        assert!(messages
            .iter()
            .all(|m| m.project == "/home/hunter/repos/test"));
        assert!(messages.iter().all(|m| m.source == "kilo"));
        // The session's model envelope is normalized to its recorded id.
        assert!(messages.iter().all(|m| m.model.as_deref() == Some("auto")));
        assert!(messages.iter().all(|m| m.timestamp.is_some()));
        assert!(!messages.iter().any(|m| m.text.contains("HIDDEN")));
    }

    fn insert_sqlite_session_and_parts(conn: &rusqlite::Connection) {
        conn.execute(
            "INSERT INTO session (id, project_id, directory, model, time_created, time_updated)
             VALUES ('ses_x', 'p', '/home/hunter/repos/cache', ?1, 1700000000000, 1700000000100)",
            rusqlite::params![r#"{"id":"auto","providerID":"freellm"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES ('m1', 'ses_x', 1700000000010, 1700000000010, '{\"role\":\"user\"}'),
                    ('m2', 'ses_x', 1700000000020, 1700000000020, '{\"role\":\"assistant\"}')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO part VALUES ('p1','m1','ses_x',1700000000011,1700000000011,'{\"type\":\"text\",\"text\":\"cached question\"}');
             INSERT INTO part VALUES ('p2','m2','ses_x',1700000000021,1700000000021,'{\"type\":\"text\",\"text\":\"cached reply\"}')",
        )
        .unwrap();
    }

    #[test]
    fn kilo_cache_reuses_messages_on_second_load() {
        use super::cache_handle;
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("kilo.db");
        let conn = open_sqlite_text_db(&db);
        insert_sqlite_session_and_parts(&conn);
        drop(conn);

        let cache_dir = tmp.path().join("cache");
        let cache = cache_handle(&cache_dir, "kilo");
        let src = KiloSource::with_cache(vec![db.clone()], cache);
        let first = src.load_messages().unwrap();
        assert_eq!(first.len(), 2);
        src.flush_cache();

        // A second run is a fresh handle over the persisted index.
        let warm = cache_handle(&cache_dir, "kilo");
        let src2 = KiloSource::with_cache(vec![db], warm.clone());
        assert_eq!(src2.load_messages().unwrap(), first);

        let stats = warm.lock().stats();
        assert!(stats.is_observed());
        assert_eq!(stats.files_seen, 1);
        assert_eq!(stats.files_reused, 1);
        assert_eq!(stats.files_extracted, 0);
        assert_eq!(stats.messages_reused, 2);
        assert_eq!(stats.messages_extracted, 0);
    }

    #[test]
    fn opencode_cache_reuses_messages_on_second_load() {
        use super::cache_handle;
        use super::opencode::OpenCodeSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("opencode.db");
        let conn = open_sqlite_text_db(&db);
        insert_sqlite_session_and_parts(&conn);
        drop(conn);

        let cache_dir = tmp.path().join("cache");
        let cache = cache_handle(&cache_dir, "opencode");
        let src = OpenCodeSource::with_cache(vec![db.clone()], cache);
        let first = src.load_messages().unwrap();
        assert_eq!(first.len(), 2);
        src.flush_cache();

        // A second run is a fresh handle over the persisted index.
        let warm = cache_handle(&cache_dir, "opencode");
        let src2 = OpenCodeSource::with_cache(vec![db], warm.clone());
        assert_eq!(src2.load_messages().unwrap(), first);

        let stats = warm.lock().stats();
        assert!(stats.is_observed());
        assert_eq!(stats.files_seen, 1);
        assert_eq!(stats.files_reused, 1);
        assert_eq!(stats.files_extracted, 0);
        assert_eq!(stats.messages_reused, 2);
        assert_eq!(stats.messages_extracted, 0);
    }

    #[test]
    fn sqlite_source_without_part_table_yields_no_messages() {
        use super::opencode::OpenCodeSource;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                  id text PRIMARY KEY,
                  project_id text NOT NULL,
                  directory text NOT NULL,
                  title text NOT NULL DEFAULT '',
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
         CREATE TABLE session_message (
                  id text PRIMARY KEY, session_id text NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, directory, time_created, time_updated)
             VALUES ('ses_y', 'p', '/p', 1700000000000, 1700000000100)",
            [],
        )
        .unwrap();
        drop(conn);

        let src = OpenCodeSource::new(vec![db]);
        // Records still load; messages degrade to empty instead of an error.
        let records = src.load().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "");
        assert!(src.load_messages().unwrap().is_empty());
    }

    #[test]
    fn sqlite_dedupes_sessions_across_dbs_without_dropping_real_messages() {
        use super::kilo::KiloSource;
        let tmp = tempfile::tempdir().unwrap();

        // `ses_dup` exists in both databases and holds two messages in the
        // first one, so a "keep every message of a claimed session" dedup would
        // also leak the copy from the second database.
        let mk = |name: &str, rows: &[(&str, &str)]| {
            let p = tmp.path().join(name);
            let c = open_sqlite_text_db(&p);
            let mut inserted_sessions: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            for (i, (sid, text)) in rows.iter().enumerate() {
                if inserted_sessions.insert(*sid) {
                    c.execute(
                        "INSERT INTO session (id, project_id, directory, time_created, time_updated)
                         VALUES (?1, 'p', '/p', 1700000000000, 1700000000100)",
                        rusqlite::params![sid],
                    )
                    .unwrap();
                }
                let mid = format!("m{i}");
                let pid = format!("p{i}");
                let t = 1700000000010 + (i as i64) * 10;
                c.execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?3, '{\"role\":\"user\"}')",
                    rusqlite::params![mid, sid, t],
                )
                .unwrap();
                c.execute(
                    "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                    rusqlite::params![pid, mid, sid, t + 1, serde_json::json!({"type": "text", "text": text}).to_string()],
                )
                .unwrap();
            }
            drop(c);
            p
        };
        let src = KiloSource::new(vec![
            mk(
                "a.db",
                &[
                    ("ses_dup", "dup in first db"),
                    ("ses_dup", "second dup in first db"),
                    ("ses_only_a", "only in first db"),
                ],
            ),
            mk(
                "b.db",
                &[
                    ("ses_dup", "dup in second db"),
                    ("ses_only_b", "only in second db"),
                ],
            ),
        ]);

        let messages = src.load_messages().unwrap();
        let got: Vec<(&str, &str)> = messages
            .iter()
            .map(|m| (m.session_id.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("ses_dup", "dup in first db"),
                ("ses_dup", "second dup in first db"),
                ("ses_only_a", "only in first db"),
                ("ses_only_b", "only in second db"),
            ],
            "the database that first claims a session keeps all of its messages \
             and drops that session's copies from later databases"
        );
    }

    #[test]
    fn registry_load_messages_all_reports_per_source_status() {
        struct AbsentMsgSource;
        impl Source for AbsentMsgSource {
            fn name(&self) -> &str {
                "missing"
            }
            fn load(&self) -> Result<Vec<Record>, SourceError> {
                Ok(Vec::new())
            }
            fn load_messages(&self) -> Result<Vec<Message>, SourceError> {
                Err(SourceError::Absent("/nonexistent".to_string()))
            }
        }
        let mut reg = Registry::new();
        reg.register(Box::new(AbsentMsgSource));
        let (messages, statuses) = reg.load_messages_all();
        assert!(messages.is_empty());
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].message_count, 0);
        assert!(matches!(
            statuses[0].error.as_ref().unwrap(),
            SourceError::Absent(_)
        ));
    }
}

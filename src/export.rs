//! Flat, machine-oriented export of normalized Records.
//!
//! `export` is the program-facing sibling of `report`: both consume the same
//! filtered `Record` set, but where `report` renders Markdown for a human,
//! this module emits one row per Record in a downstream-friendly format.
//! The seam is the pure [`render`] function — no Source access, no stdout —
//! so it is unit-testable with hand-built `Record` values.
//!
//! With `--messages` the same machinery exports one row per [`Message`]
//! instead: the transcript corpus `search` reads. [`render_messages`] is the
//! message-side seam, likewise pure and Source-free. The two paths share the
//! format enum and the JSON/CSV/TSV emit helpers but keep separate field
//! enums, so a name valid for one unit is a loud error for the other.

use std::io::Write;

use crate::domain::message::Message;
use crate::domain::record::Record;

/// Output encoding selected by `--format`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportFormat {
    /// One compact JSON object per Record, one per line.
    #[default]
    Jsonl,
    /// A single pretty-printed JSON array.
    Json,
    /// Comma-separated, with a header row.
    Csv,
    /// Tab-separated, with a header row.
    Tsv,
}

/// A selectable column in an exported row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordField {
    Source,
    SessionId,
    Project,
    Model,
    Agent,
    StartedAt,
    EndedAt,
    Messages,
    Input,
    Output,
    CacheRead,
    CacheWrite,
    Cost,
}

impl RecordField {
    /// The canonical column order, applied when `--fields` is omitted.
    pub const ALL: [RecordField; 13] = [
        RecordField::Source,
        RecordField::SessionId,
        RecordField::Project,
        RecordField::Model,
        RecordField::Agent,
        RecordField::StartedAt,
        RecordField::EndedAt,
        RecordField::Messages,
        RecordField::Input,
        RecordField::Output,
        RecordField::CacheRead,
        RecordField::CacheWrite,
        RecordField::Cost,
    ];

    /// The flag spelling of this field, also used as the JSON key and the
    /// CSV/TSV header.
    pub fn name(self) -> &'static str {
        match self {
            RecordField::Source => "source",
            RecordField::SessionId => "session_id",
            RecordField::Project => "project",
            RecordField::Model => "model",
            RecordField::Agent => "agent",
            RecordField::StartedAt => "started_at",
            RecordField::EndedAt => "ended_at",
            RecordField::Messages => "messages",
            RecordField::Input => "input",
            RecordField::Output => "output",
            RecordField::CacheRead => "cache_read",
            RecordField::CacheWrite => "cache_write",
            RecordField::Cost => "cost",
        }
    }

    /// Parse a single `--fields` token. Case-sensitive: the names are a
    /// contract with scripts, so a casing typo should fail loudly.
    pub fn parse(s: &str) -> Option<RecordField> {
        RecordField::ALL.into_iter().find(|f| f.name() == s)
    }
}

/// Fully resolved emission options: the target format and the ordered field
/// projection. Callers build this once from CLI args, so [`render`] never
/// reparses flag strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub fields: Vec<RecordField>,
}

impl ExportOptions {
    /// Resolve the `--fields` tokens (already flattened from their CSV/repeat
    /// forms) into an ordered field list. An empty slice selects the canonical
    /// all-fields order. An unknown name is an error that names the offending
    /// token and lists the valid names.
    pub fn resolve(fields: &[String]) -> anyhow::Result<Vec<RecordField>> {
        if fields.is_empty() {
            return Ok(RecordField::ALL.to_vec());
        }
        fields
            .iter()
            .map(|s| {
                RecordField::parse(s).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown field '{}'; valid fields: {}",
                        s,
                        RecordField::ALL
                            .iter()
                            .map(|f| f.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
            })
            .collect()
    }
}

/// One Record projected into the selected fields.
///
/// The projection is a plain ordered `Vec` of `(name, JSON value)` pairs
/// rather than a serde-derived struct, because the object key order must
/// follow `--fields` exactly and `serde_json::Map` does not preserve
/// insertion order without the `preserve_order` feature. Absent optional
/// values become `Value::Null` so they render as JSON `null` and as an empty
/// CSV/TSV cell rather than a misleading `0`.
fn project(r: &Record, fields: &[RecordField]) -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::Value;
    fields
        .iter()
        .map(|f| {
            let v = match f {
                RecordField::Source => Value::String(r.source.clone()),
                RecordField::SessionId => Value::String(r.session_id.clone()),
                RecordField::Project => Value::String(r.project.clone()),
                RecordField::Model => Value::String(r.model.clone()),
                RecordField::Agent => match &r.agent {
                    Some(a) => Value::String(a.clone()),
                    None => Value::Null,
                },
                RecordField::StartedAt => Value::String(r.started_at.to_rfc3339()),
                RecordField::EndedAt => match r.ended_at {
                    Some(e) => Value::String(e.to_rfc3339()),
                    None => Value::Null,
                },
                RecordField::Messages => Value::from(r.message_count),
                RecordField::Input => Value::from(r.tokens.input),
                RecordField::Output => Value::from(r.tokens.output),
                RecordField::CacheRead => Value::from(r.tokens.cache_read),
                RecordField::CacheWrite => Value::from(r.tokens.cache_write),
                RecordField::Cost => match r.cost {
                    Some(c) => Value::from(c),
                    None => Value::Null,
                },
            };
            (f.name(), v)
        })
        .collect()
}

/// A cell's text for CSV/TSV. Raw integers, six-decimal cost (matching the
/// `sessions`/`diff` CSV convention), empty when the value is absent.
/// Deliberately not `format_tokens` — a spreadsheet should see numbers, not
/// `12.3M`.
fn cell(field: RecordField, value: &serde_json::Value) -> String {
    match field {
        RecordField::Cost => match value.as_f64() {
            Some(c) => format!("{:.6}", c),
            None => String::new(),
        },
        _ => match value {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            other => other.to_string(),
        },
    }
}

/// Render the given Records in the requested format, in a deterministic
/// order: `started_at` descending, tie-broken by `(source, session_id)`
/// ascending, so two exports of identical data are byte-identical.
///
/// This is the module's one seam: it touches no Source and writes only to
/// `out`.
pub fn render<W: Write>(
    records: &[Record],
    opts: &ExportOptions,
    out: &mut W,
) -> anyhow::Result<()> {
    let mut sorted: Vec<&Record> = records.iter().collect();
    sorted.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| (&a.source, &a.session_id).cmp(&(&b.source, &b.session_id)))
    });

    match opts.format {
        ExportFormat::Jsonl => render_jsonl(&sorted, opts, out),
        ExportFormat::Json => render_json(&sorted, opts, out),
        ExportFormat::Csv => render_delimited(&sorted, opts, b',', out),
        ExportFormat::Tsv => render_delimited(&sorted, opts, b'\t', out),
    }
}

/// Serialize one projected row as a compact JSON object with keys in field
/// order. `serde_json::Map` cannot be used here: without `preserve_order` it
/// is a BTreeMap and would reorder the keys alphabetically.
fn row_to_json(pairs: &[(&'static str, serde_json::Value)]) -> serde_json::Result<String> {
    let mut s = String::from("{");
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&serde_json::to_string(k)?);
        s.push(':');
        s.push_str(&serde_json::to_string(v)?);
    }
    s.push('}');
    Ok(s)
}

fn render_jsonl<W: Write>(
    records: &[&Record],
    opts: &ExportOptions,
    out: &mut W,
) -> anyhow::Result<()> {
    for r in records {
        let row = project(r, &opts.fields);
        writeln!(out, "{}", row_to_json(&row)?)?;
    }
    Ok(())
}

fn render_json<W: Write>(
    records: &[&Record],
    opts: &ExportOptions,
    out: &mut W,
) -> anyhow::Result<()> {
    if records.is_empty() {
        writeln!(out, "[]")?;
        return Ok(());
    }
    writeln!(out, "[")?;
    for (i, r) in records.iter().enumerate() {
        let row = project(r, &opts.fields);
        let comma = if i + 1 == records.len() { "" } else { "," };
        writeln!(out, "  {}{}", row_to_json(&row)?, comma)?;
    }
    writeln!(out, "]")?;
    Ok(())
}

fn render_delimited<W: Write>(
    records: &[&Record],
    opts: &ExportOptions,
    delimiter: u8,
    out: &mut W,
) -> anyhow::Result<()> {
    let mut w = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(out);
    w.write_record(opts.fields.iter().map(|f| f.name()))?;
    for r in records {
        let row = project(r, &opts.fields);
        let record: Vec<String> = row
            .iter()
            .zip(opts.fields.iter())
            .map(|((_, v), f)| cell(*f, v))
            .collect();
        w.write_record(&record)?;
    }
    w.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Message-level export (`export --messages`)
// ---------------------------------------------------------------------------

/// A selectable column in an exported Message row.
///
/// Deliberately a separate enum from [`RecordField`]: the two units share some
/// names (`source`, `session_id`, `project`, `model`) but not all, and a single
/// merged enum would let `--fields cost` validate in message mode even though a
/// `Message` has no cost. Keeping them apart makes a wrong-unit name a precise
/// error rather than a silent null column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageField {
    Source,
    SessionId,
    Project,
    Model,
    Role,
    Timestamp,
    Text,
}

impl MessageField {
    /// The canonical column order, applied when `--fields` is omitted.
    /// Identity and location first, the payload `text` last — mirroring how
    /// Record export puts the derived `cost` last.
    pub const ALL: [MessageField; 7] = [
        MessageField::Source,
        MessageField::SessionId,
        MessageField::Project,
        MessageField::Model,
        MessageField::Role,
        MessageField::Timestamp,
        MessageField::Text,
    ];

    /// The flag spelling of this field, also used as the JSON key and the
    /// CSV/TSV header.
    pub fn name(self) -> &'static str {
        match self {
            MessageField::Source => "source",
            MessageField::SessionId => "session_id",
            MessageField::Project => "project",
            MessageField::Model => "model",
            MessageField::Role => "role",
            MessageField::Timestamp => "timestamp",
            MessageField::Text => "text",
        }
    }

    /// Parse a single `--fields` token against the message field set.
    /// Case-sensitive, like the Record counterpart: the names are a contract
    /// with scripts, so a casing typo should fail loudly.
    pub fn parse(s: &str) -> Option<MessageField> {
        MessageField::ALL.into_iter().find(|f| f.name() == s)
    }
}

/// Fully resolved message emission options, mirroring [`ExportOptions`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageExportOptions {
    pub format: ExportFormat,
    pub fields: Vec<MessageField>,
}

impl MessageExportOptions {
    /// Resolve `--fields` tokens into an ordered message field list. An empty
    /// slice selects the canonical order. An unknown name is an error that
    /// names the offending token and lists the valid **message** field names.
    pub fn resolve(fields: &[String]) -> anyhow::Result<Vec<MessageField>> {
        if fields.is_empty() {
            return Ok(MessageField::ALL.to_vec());
        }
        fields
            .iter()
            .map(|s| {
                MessageField::parse(s).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown field '{}' for --messages; valid fields: {}",
                        s,
                        MessageField::ALL
                            .iter()
                            .map(|f| f.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
            })
            .collect()
    }
}

/// One Message projected into the selected fields. Same shape and rationale as
/// [`project`]: an ordered `Vec` of `(name, value)` pairs so key order follows
/// `--fields`, with absent `model`/`timestamp` as `Value::Null`.
fn project_message(m: &Message, fields: &[MessageField]) -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::Value;
    fields
        .iter()
        .map(|f| {
            let v = match f {
                MessageField::Source => Value::String(m.source.clone()),
                MessageField::SessionId => Value::String(m.session_id.clone()),
                MessageField::Project => Value::String(m.project.clone()),
                MessageField::Model => match &m.model {
                    Some(model) => Value::String(model.clone()),
                    None => Value::Null,
                },
                MessageField::Role => Value::String(m.role.clone()),
                MessageField::Timestamp => match m.timestamp {
                    Some(t) => Value::String(t.to_rfc3339()),
                    None => Value::Null,
                },
                MessageField::Text => Value::String(m.text.clone()),
            };
            (f.name(), v)
        })
        .collect()
}

/// Render the given Messages in the requested format, in a deterministic
/// order: grouped by `(source, session_id)` ascending, then `timestamp`
/// ascending with a missing timestamp sorted last within its session, then the
/// original load order as the final tiebreak. Messages read as a transcript, so
/// within a session they come out in the order they happened; across sessions
/// the grouping makes two exports byte-identical.
///
/// The message-side counterpart of [`render`]: it touches no Source and writes
/// only to `out`.
pub fn render_messages<W: Write>(
    messages: &[Message],
    opts: &MessageExportOptions,
    out: &mut W,
) -> anyhow::Result<()> {
    let mut sorted: Vec<&Message> = messages.iter().collect();
    sorted.sort_by(|a, b| {
        (&a.source, &a.session_id)
            .cmp(&(&b.source, &b.session_id))
            .then_with(|| match (a.timestamp, b.timestamp) {
                (Some(x), Some(y)) => x.cmp(&y),
                // A missing timestamp sorts after any present one, keeping an
                // untimed message at the end of its own session.
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
    });

    match opts.format {
        ExportFormat::Jsonl => render_messages_jsonl(&sorted, opts, out),
        ExportFormat::Json => render_messages_json(&sorted, opts, out),
        ExportFormat::Csv => render_messages_delimited(&sorted, opts, b',', out),
        ExportFormat::Tsv => render_messages_delimited(&sorted, opts, b'\t', out),
    }
}

fn render_messages_jsonl<W: Write>(
    messages: &[&Message],
    opts: &MessageExportOptions,
    out: &mut W,
) -> anyhow::Result<()> {
    for m in messages {
        let row = project_message(m, &opts.fields);
        writeln!(out, "{}", row_to_json(&row)?)?;
    }
    Ok(())
}

fn render_messages_json<W: Write>(
    messages: &[&Message],
    opts: &MessageExportOptions,
    out: &mut W,
) -> anyhow::Result<()> {
    if messages.is_empty() {
        writeln!(out, "[]")?;
        return Ok(());
    }
    writeln!(out, "[")?;
    for (i, m) in messages.iter().enumerate() {
        let row = project_message(m, &opts.fields);
        let comma = if i + 1 == messages.len() { "" } else { "," };
        writeln!(out, "  {}{}", row_to_json(&row)?, comma)?;
    }
    writeln!(out, "]")?;
    Ok(())
}

fn render_messages_delimited<W: Write>(
    messages: &[&Message],
    opts: &MessageExportOptions,
    delimiter: u8,
    out: &mut W,
) -> anyhow::Result<()> {
    let mut w = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(out);
    w.write_record(opts.fields.iter().map(|f| f.name()))?;
    for m in messages {
        let row = project_message(m, &opts.fields);
        let record: Vec<String> = row.iter().map(|(_, v)| message_cell(v)).collect();
        w.write_record(&record)?;
    }
    w.flush()?;
    Ok(())
}

/// A message cell's text for CSV/TSV. Nulls become empty cells (never `0`);
/// strings — including multi-line `text` — are passed through and quoted by the
/// `csv` writer. No message field has a special numeric rendering, so unlike
/// [`cell`] there is no cost branch, and this takes no field.
fn message_cell(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use chrono::{TimeZone, Utc};

    fn ts(secs: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn rec(source: &str, id: &str, started: i64) -> Record {
        Record {
            session_id: id.to_string(),
            source: source.to_string(),
            project: "/home/user/proj".to_string(),
            model: "auto".to_string(),
            agent: Some("build".to_string()),
            started_at: ts(started),
            ended_at: Some(ts(started + 60)),
            tokens: TokenBreakdown {
                input: 1_234,
                output: 567,
                cache_read: 89,
                cache_write: 12,
            },
            message_count: 4,
            cost: Some(0.5),
        }
    }

    fn opts(format: ExportFormat, fields: &[RecordField]) -> ExportOptions {
        ExportOptions {
            format,
            fields: fields.to_vec(),
        }
    }

    fn all_fields() -> Vec<RecordField> {
        RecordField::ALL.to_vec()
    }

    fn run(records: &[Record], o: &ExportOptions) -> String {
        let mut buf = Vec::new();
        render(records, o, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    /// Assert the first JSON object in `doc` carries exactly `expected` keys,
    /// in that order. Uses each key's `"name":` token position, which is
    /// robust to values containing commas or quotes, unlike parsing the
    /// parsed `serde_json::Map` (which sorts keys alphabetically).
    fn assert_key_order(doc: &str, expected: &[&str]) {
        let line = doc
            .lines()
            .map(|l| l.trim().trim_end_matches(','))
            .find(|l| l.starts_with('{'))
            .expect("no JSON object found");
        let mut positions = Vec::new();
        for key in expected {
            let needle = format!("\"{}\":", key);
            let pos = line.find(&needle).unwrap_or_else(|| {
                panic!("key {key} not found in: {line}");
            });
            positions.push(pos);
        }
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        assert_eq!(positions, sorted, "keys out of order in: {line}");
        // And no unrequested keys: count `":` occurrences loosely by parsing.
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            parsed.as_object().unwrap().len(),
            expected.len(),
            "unexpected extra keys in: {line}"
        );
    }

    #[test]
    fn jsonl_emits_one_line_per_record() {
        let records = vec![rec("claude", "s1", 100), rec("omp", "s2", 200)];
        let out = run(&records, &opts(ExportFormat::Jsonl, &all_fields()));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.is_object());
        }
    }

    #[test]
    fn jsonl_is_compact_one_object_per_line() {
        let out = run(
            &[rec("claude", "s1", 100), rec("omp", "s2", 200)],
            &opts(ExportFormat::Jsonl, &all_fields()),
        );
        // One trailing newline per record, no pretty-printing inside a line.
        assert_eq!(out.lines().count(), 2);
        assert!(out.ends_with('\n'));
        for line in out.lines() {
            assert!(!line.starts_with(' '));
            assert!(!line.contains("  "));
        }
    }

    #[test]
    fn canonical_field_order_is_stable() {
        let out = run(
            &[rec("claude", "s1", 100)],
            &opts(ExportFormat::Json, &all_fields()),
        );
        assert_key_order(
            &out,
            &[
                "source",
                "session_id",
                "project",
                "model",
                "agent",
                "started_at",
                "ended_at",
                "messages",
                "input",
                "output",
                "cache_read",
                "cache_write",
                "cost",
            ],
        );
    }

    #[test]
    fn fields_subset_and_reorder() {
        let fields = vec![
            RecordField::Input,
            RecordField::Source,
            RecordField::Project,
        ];
        let out = run(
            &[rec("claude", "s1", 100)],
            &opts(ExportFormat::Json, &fields),
        );
        assert_key_order(&out, &["input", "source", "project"]);
    }

    #[test]
    fn absent_optionals_render_null_in_json_and_empty_in_csv() {
        let mut r = rec("claude", "s1", 100);
        r.agent = None;
        r.ended_at = None;
        r.cost = None;
        let json = run(&[r.clone()], &opts(ExportFormat::Json, &all_fields()));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = v.as_array().unwrap()[0].as_object().unwrap();
        assert!(obj["agent"].is_null());
        assert!(obj["ended_at"].is_null());
        assert!(obj["cost"].is_null());

        let csv = run(&[r], &opts(ExportFormat::Csv, &all_fields()));
        let row = csv.lines().nth(1).unwrap();
        let cols: Vec<&str> = row.split(',').collect();
        assert_eq!(cols.len(), 13);
        // agent, ended_at, cost are empty cells, not "0" or "null".
        assert_eq!(cols[4], "", "agent should be empty");
        assert_eq!(cols[6], "", "ended_at should be empty");
        assert_eq!(cols[12], "", "cost should be empty");
        assert!(!row.contains("null"));
    }

    #[test]
    fn csv_has_header_and_one_row_per_record() {
        let records = vec![rec("claude", "s1", 100), rec("omp", "s2", 200)];
        let out = run(&records, &opts(ExportFormat::Csv, &all_fields()));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "source,session_id,project,model,agent,started_at,ended_at,messages,input,output,cache_read,cache_write,cost");
    }

    #[test]
    fn tsv_uses_tab_delimiter() {
        let out = run(
            &[rec("claude", "s1", 100)],
            &opts(ExportFormat::Tsv, &all_fields()),
        );
        let header = out.lines().next().unwrap();
        assert!(header.contains('\t'));
        assert!(!header.contains(','));
        assert_eq!(header.split('\t').count(), 13);
    }

    #[test]
    fn token_cells_are_raw_integers_not_magnitudes() {
        let out = run(
            &[rec("claude", "s1", 100)],
            &opts(ExportFormat::Csv, &all_fields()),
        );
        let row = out.lines().nth(1).unwrap();
        assert!(row.contains("1234"));
        assert!(!row.contains("1.2K"));
    }

    #[test]
    fn cost_cell_keeps_six_decimals() {
        let mut r = rec("omp", "s1", 100);
        r.cost = Some(1.2345678);
        let csv = run(&[r.clone()], &opts(ExportFormat::Csv, &all_fields()));
        assert!(csv.lines().nth(1).unwrap().contains("1.234568"));
        // JSON keeps the raw value, not the padded form.
        let json = run(&[r], &opts(ExportFormat::Jsonl, &all_fields()));
        let v: serde_json::Value = serde_json::from_str(json.lines().next().unwrap()).unwrap();
        assert_eq!(v["cost"], 1.2345678);
    }

    #[test]
    fn deterministic_order_started_desc_then_identity() {
        let records = vec![
            rec("omp", "s2", 100),
            rec("claude", "s1", 200),
            rec("omp", "s1", 200),
        ];
        let out = run(&records, &opts(ExportFormat::Jsonl, &all_fields()));
        let ids: Vec<String> = out
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                format!(
                    "{}/{}",
                    v["source"].as_str().unwrap(),
                    v["session_id"].as_str().unwrap()
                )
            })
            .collect();
        // 200 first (claude/s1 before omp/s1), then 100 (omp/s2)
        assert_eq!(ids, vec!["claude/s1", "omp/s1", "omp/s2"]);
    }

    #[test]
    fn empty_jsonl_is_empty() {
        assert_eq!(run(&[], &opts(ExportFormat::Jsonl, &all_fields())), "");
    }

    #[test]
    fn empty_json_is_array() {
        let out = run(&[], &opts(ExportFormat::Json, &all_fields()));
        assert_eq!(out.trim(), "[]");
    }

    #[test]
    fn empty_csv_is_header_only() {
        let out = run(&[], &opts(ExportFormat::Csv, &all_fields()));
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn resolve_empty_selects_all() {
        assert_eq!(
            ExportOptions::resolve(&[]).unwrap(),
            RecordField::ALL.to_vec()
        );
    }

    #[test]
    fn resolve_unknown_field_errors_and_names_it() {
        let err = ExportOptions::resolve(&["source".into(), "bogus".into()]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bogus"), "got: {msg}");
        assert!(msg.contains("valid fields"), "got: {msg}");
    }

    #[test]
    fn resolve_preserves_order_and_duplicates() {
        let fields = ExportOptions::resolve(&["cost".into(), "source".into()]).unwrap();
        assert_eq!(fields, vec![RecordField::Cost, RecordField::Source]);
    }

    #[test]
    fn field_parse_is_case_sensitive() {
        assert_eq!(RecordField::parse("source"), Some(RecordField::Source));
        assert_eq!(RecordField::parse("Source"), None);
    }

    // -- message export ----------------------------------------------------

    fn msg(source: &str, session: &str, role: &str, secs: Option<i64>) -> Message {
        Message {
            source: source.to_string(),
            session_id: session.to_string(),
            project: "/home/user/proj".to_string(),
            model: Some("auto".to_string()),
            role: role.to_string(),
            timestamp: secs.map(ts),
            text: format!("{role} says hi"),
        }
    }

    fn msg_opts(format: ExportFormat, fields: &[MessageField]) -> MessageExportOptions {
        MessageExportOptions {
            format,
            fields: fields.to_vec(),
        }
    }

    fn all_msg_fields() -> Vec<MessageField> {
        MessageField::ALL.to_vec()
    }

    fn run_msgs(messages: &[Message], o: &MessageExportOptions) -> String {
        let mut buf = Vec::new();
        render_messages(messages, o, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn message_jsonl_one_object_per_line() {
        let msgs = vec![
            msg("claude", "s1", "user", Some(100)),
            msg("omp", "s2", "assistant", Some(200)),
        ];
        let out = run_msgs(&msgs, &msg_opts(ExportFormat::Jsonl, &all_msg_fields()));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.is_object());
        }
    }

    #[test]
    fn message_canonical_field_order_is_stable() {
        let out = run_msgs(
            &[msg("claude", "s1", "user", Some(100))],
            &msg_opts(ExportFormat::Json, &all_msg_fields()),
        );
        assert_key_order(
            &out,
            &[
                "source",
                "session_id",
                "project",
                "model",
                "role",
                "timestamp",
                "text",
            ],
        );
    }

    #[test]
    fn message_fields_subset_and_reorder() {
        let fields = vec![MessageField::Role, MessageField::Text];
        let out = run_msgs(
            &[msg("claude", "s1", "user", Some(100))],
            &msg_opts(ExportFormat::Json, &fields),
        );
        assert_key_order(&out, &["role", "text"]);
    }

    #[test]
    fn message_absent_model_and_timestamp_are_null_then_empty() {
        let mut m = msg("claude", "s1", "user", None);
        m.model = None;
        let json = run_msgs(
            &[m.clone()],
            &msg_opts(ExportFormat::Json, &all_msg_fields()),
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = v.as_array().unwrap()[0].as_object().unwrap();
        assert!(obj["model"].is_null());
        assert!(obj["timestamp"].is_null());

        let csv = run_msgs(&[m], &msg_opts(ExportFormat::Csv, &all_msg_fields()));
        let row = csv.lines().nth(1).unwrap();
        // csv quotes fields containing commas; this row has none, so a plain
        // split is safe. model cols[3], timestamp cols[5] are empty.
        let cols: Vec<&str> = row.split(',').collect();
        assert_eq!(cols[3], "", "model should be empty");
        assert_eq!(cols[5], "", "timestamp should be empty");
        assert!(!row.contains("null"));
    }

    #[test]
    fn message_csv_header_and_delimiter() {
        let csv = run_msgs(
            &[msg("claude", "s1", "user", Some(100))],
            &msg_opts(ExportFormat::Csv, &all_msg_fields()),
        );
        assert_eq!(
            csv.lines().next().unwrap(),
            "source,session_id,project,model,role,timestamp,text"
        );
        let tsv = run_msgs(
            &[msg("claude", "s1", "user", Some(100))],
            &msg_opts(ExportFormat::Tsv, &all_msg_fields()),
        );
        let header = tsv.lines().next().unwrap();
        assert!(header.contains('\t'));
        assert!(!header.contains(','));
        assert_eq!(header.split('\t').count(), 7);
    }

    #[test]
    fn message_csv_quotes_multiline_text() {
        let mut m = msg("claude", "s1", "user", Some(100));
        m.text = "line one\nline two, with comma".to_string();
        let csv = run_msgs(&[m], &msg_opts(ExportFormat::Csv, &all_msg_fields()));
        // The csv writer quotes a value containing a comma and keeps the
        // embedded newline inside quotes.
        assert!(
            csv.contains("\"line one\nline two, with comma\""),
            "got: {csv}"
        );
    }

    #[test]
    fn message_empty_set_shapes() {
        assert_eq!(
            run_msgs(&[], &msg_opts(ExportFormat::Json, &all_msg_fields())).trim(),
            "[]"
        );
        assert_eq!(
            run_msgs(&[], &msg_opts(ExportFormat::Jsonl, &all_msg_fields())),
            ""
        );
        assert_eq!(
            run_msgs(&[], &msg_opts(ExportFormat::Csv, &all_msg_fields()))
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn message_ordering_is_chronological_within_session_none_last() {
        // Same session, handed in out of order with a None-timestamp row.
        let msgs = vec![
            msg("claude", "s1", "assistant", Some(300)),
            msg("claude", "s1", "user", None),
            msg("claude", "s1", "user", Some(100)),
            msg("claude", "s1", "assistant", Some(200)),
        ];
        let out = run_msgs(
            &msgs,
            &msg_opts(ExportFormat::Jsonl, &[MessageField::Timestamp]),
        );
        let stamps: Vec<serde_json::Value> = out
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["timestamp"].clone())
            .collect();
        // 100, 200, 300 present ascending, then the None row.
        assert_eq!(stamps[0].as_str().unwrap(), ts(100).to_rfc3339());
        assert_eq!(stamps[1].as_str().unwrap(), ts(200).to_rfc3339());
        assert_eq!(stamps[2].as_str().unwrap(), ts(300).to_rfc3339());
        assert!(stamps[3].is_null(), "None timestamp must sort last");
    }

    #[test]
    fn message_ordering_groups_by_source_then_session() {
        let msgs = vec![
            msg("omp", "s2", "user", Some(100)),
            msg("claude", "s9", "user", Some(100)),
            msg("claude", "s1", "user", Some(100)),
            msg("claude", "s1", "assistant", Some(100)),
        ];
        let out = run_msgs(
            &msgs,
            &msg_opts(
                ExportFormat::Jsonl,
                &[MessageField::Source, MessageField::SessionId],
            ),
        );
        let keys: Vec<(String, String)> = out
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                (
                    v["source"].as_str().unwrap().to_string(),
                    v["session_id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("claude".into(), "s1".into()),
                ("claude".into(), "s1".into()),
                ("claude".into(), "s9".into()),
                ("omp".into(), "s2".into()),
            ]
        );
    }

    #[test]
    fn message_resolve_empty_selects_all() {
        assert_eq!(
            MessageExportOptions::resolve(&[]).unwrap(),
            MessageField::ALL.to_vec()
        );
    }

    #[test]
    fn message_resolve_unknown_names_it_as_message_field() {
        let err = MessageExportOptions::resolve(&["role".into(), "cost".into()]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cost"), "got: {msg}");
        assert!(msg.contains("--messages"), "got: {msg}");
        assert!(msg.contains("valid fields"), "got: {msg}");
    }

    #[test]
    fn message_field_parse_rejects_record_only_name() {
        assert_eq!(MessageField::parse("role"), Some(MessageField::Role));
        assert_eq!(MessageField::parse("cost"), None);
        assert_eq!(MessageField::parse("Role"), None);
    }
}

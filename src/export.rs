//! Flat, machine-oriented export of normalized Records.
//!
//! `export` is the program-facing sibling of `report`: both consume the same
//! filtered `Record` set, but where `report` renders Markdown for a human,
//! this module emits one row per Record in a downstream-friendly format.
//! The seam is the pure [`render`] function — no Source access, no stdout —
//! so it is unit-testable with hand-built `Record` values.

use std::io::Write;

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
pub enum Field {
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

impl Field {
    /// The canonical column order, applied when `--fields` is omitted.
    pub const ALL: [Field; 13] = [
        Field::Source,
        Field::SessionId,
        Field::Project,
        Field::Model,
        Field::Agent,
        Field::StartedAt,
        Field::EndedAt,
        Field::Messages,
        Field::Input,
        Field::Output,
        Field::CacheRead,
        Field::CacheWrite,
        Field::Cost,
    ];

    /// The flag spelling of this field, also used as the JSON key and the
    /// CSV/TSV header.
    pub fn name(self) -> &'static str {
        match self {
            Field::Source => "source",
            Field::SessionId => "session_id",
            Field::Project => "project",
            Field::Model => "model",
            Field::Agent => "agent",
            Field::StartedAt => "started_at",
            Field::EndedAt => "ended_at",
            Field::Messages => "messages",
            Field::Input => "input",
            Field::Output => "output",
            Field::CacheRead => "cache_read",
            Field::CacheWrite => "cache_write",
            Field::Cost => "cost",
        }
    }

    /// Parse a single `--fields` token. Case-sensitive: the names are a
    /// contract with scripts, so a casing typo should fail loudly.
    pub fn parse(s: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.name() == s)
    }
}

/// Fully resolved emission options: the target format and the ordered field
/// projection. Callers build this once from CLI args, so [`render`] never
/// reparses flag strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub fields: Vec<Field>,
}

impl ExportOptions {
    /// Resolve the `--fields` tokens (already flattened from their CSV/repeat
    /// forms) into an ordered field list. An empty slice selects the canonical
    /// all-fields order. An unknown name is an error that names the offending
    /// token and lists the valid names.
    pub fn resolve(fields: &[String]) -> anyhow::Result<Vec<Field>> {
        if fields.is_empty() {
            return Ok(Field::ALL.to_vec());
        }
        fields
            .iter()
            .map(|s| {
                Field::parse(s).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown field '{}'; valid fields: {}",
                        s,
                        Field::ALL
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
fn project(r: &Record, fields: &[Field]) -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::Value;
    fields
        .iter()
        .map(|f| {
            let v = match f {
                Field::Source => Value::String(r.source.clone()),
                Field::SessionId => Value::String(r.session_id.clone()),
                Field::Project => Value::String(r.project.clone()),
                Field::Model => Value::String(r.model.clone()),
                Field::Agent => match &r.agent {
                    Some(a) => Value::String(a.clone()),
                    None => Value::Null,
                },
                Field::StartedAt => Value::String(r.started_at.to_rfc3339()),
                Field::EndedAt => match r.ended_at {
                    Some(e) => Value::String(e.to_rfc3339()),
                    None => Value::Null,
                },
                Field::Messages => Value::from(r.message_count),
                Field::Input => Value::from(r.tokens.input),
                Field::Output => Value::from(r.tokens.output),
                Field::CacheRead => Value::from(r.tokens.cache_read),
                Field::CacheWrite => Value::from(r.tokens.cache_write),
                Field::Cost => match r.cost {
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
fn cell(field: Field, value: &serde_json::Value) -> String {
    match field {
        Field::Cost => match value.as_f64() {
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

    fn opts(format: ExportFormat, fields: &[Field]) -> ExportOptions {
        ExportOptions {
            format,
            fields: fields.to_vec(),
        }
    }

    fn all_fields() -> Vec<Field> {
        Field::ALL.to_vec()
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
        let fields = vec![Field::Input, Field::Source, Field::Project];
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
        assert_eq!(ExportOptions::resolve(&[]).unwrap(), Field::ALL.to_vec());
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
        assert_eq!(fields, vec![Field::Cost, Field::Source]);
    }

    #[test]
    fn field_parse_is_case_sensitive() {
        assert_eq!(Field::parse("source"), Some(Field::Source));
        assert_eq!(Field::parse("Source"), None);
    }
}

use std::io::Write;

use serde::Serialize;

use crate::aggregator::{AggregateResult, Group};
use crate::diff::{DiffRow, Presence};
use crate::source::SourceStatus;

#[derive(Clone, Debug, Serialize)]
struct SourceInfo {
    name: String,
    records: usize,
    status: String,
}

#[derive(Clone, Debug, Serialize)]
struct JsonPayload {
    sources: Vec<SourceInfo>,
    group_by: String,
    groups: Vec<Group>,
}

pub struct OutputRenderer;

impl OutputRenderer {
    pub fn json<W: Write>(
        &self,
        groups: &[Group],
        source_statuses: &[SourceStatus],
        group_by: &str,
        out: &mut W,
    ) -> anyhow::Result<()> {
        let sources: Vec<SourceInfo> = source_statuses
            .iter()
            .map(|s| SourceInfo {
                name: s.name.clone(),
                records: s.record_count,
                status: match &s.error {
                    None => "ok".to_string(),
                    Some(e) => format!("{}", e),
                },
            })
            .collect();
        let payload = JsonPayload {
            sources,
            group_by: group_by.to_string(),
            groups: groups.to_vec(),
        };
        serde_json::to_writer_pretty(out, &payload)?;
        Ok(())
    }

    pub fn csv<W: Write>(&self, groups: &[Group], out: &mut W) -> anyhow::Result<()> {
        let mut w = csv::Writer::from_writer(out);
        w.write_record([
            "group_key",
            "source",
            "sessions",
            "messages",
            "input",
            "output",
            "cache_read",
            "cache_write",
            "cost",
        ])?;
        for g in groups {
            w.write_record([
                &g.key,
                &g.source,
                &g.sessions.to_string(),
                &g.messages.to_string(),
                &g.tokens.input.to_string(),
                &g.tokens.output.to_string(),
                &g.tokens.cache_read.to_string(),
                &g.tokens.cache_write.to_string(),
                &g.cost.map(|c| format!("{:.6}", c)).unwrap_or_default(),
            ])?;
        }
        w.flush()?;
        Ok(())
    }
}

// --- Diff rendering ---

#[derive(Clone, Debug, Serialize)]
struct DiffWindow {
    since: String,
    until: String,
}

#[derive(Clone, Debug, Serialize)]
struct DiffPayload {
    windows: DiffWindows,
    sources: Vec<SourceInfo>,
    group_by: String,
    rows: Vec<DiffRow>,
}

#[derive(Clone, Debug, Serialize)]
struct DiffWindows {
    previous: DiffWindow,
    current: DiffWindow,
}

fn snap_cell(v: u64) -> String {
    v.to_string()
}

fn opt_snap_cell(v: Option<u64>) -> String {
    v.map(snap_cell).unwrap_or_default()
}

fn pct_cell(v: Option<f64>) -> String {
    v.map(|p| format!("{:.1}%", p)).unwrap_or_default()
}

fn cost_cell(v: Option<f64>) -> String {
    v.map(|c| format!("{:.6}", c)).unwrap_or_default()
}

/// Emit diff as JSON.
pub fn render_diff_json(
    rows: &[DiffRow],
    source_statuses: &[SourceStatus],
    group_by: &str,
    prev_since: chrono::DateTime<chrono::Utc>,
    prev_until: chrono::DateTime<chrono::Utc>,
    curr_since: chrono::DateTime<chrono::Utc>,
    curr_until: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<()> {
    let payload = DiffPayload {
        windows: DiffWindows {
            previous: DiffWindow {
                since: prev_since.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                until: prev_until.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            },
            current: DiffWindow {
                since: curr_since.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                until: curr_until.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            },
        },
        sources: source_statuses
            .iter()
            .map(|s| SourceInfo {
                name: s.name.clone(),
                records: s.record_count,
                status: match &s.error {
                    None => "ok".to_string(),
                    Some(e) => format!("{}", e),
                },
            })
            .collect(),
        group_by: group_by.to_string(),
        rows: rows.to_vec(),
    };

    let mut buf = Vec::new();
    serde_json::to_writer_pretty(&mut buf, &payload)?;
    println!("{}", String::from_utf8(buf)?);
    Ok(())
}

/// Emit diff as CSV.
pub fn render_diff_csv(
    rows: &[DiffRow],
    _source_statuses: &[SourceStatus],
    _prev_agg: &AggregateResult,
    _curr_agg: &AggregateResult,
) -> anyhow::Result<()> {
    let mut w = csv::Writer::from_writer(std::io::stdout());
    w.write_record([
        "group_key",
        "presence",
        "prev_sessions",
        "curr_sessions",
        "delta_sessions",
        "prev_messages",
        "curr_messages",
        "delta_messages",
        "prev_input",
        "curr_input",
        "delta_input",
        "delta_pct_input",
        "prev_output",
        "curr_output",
        "delta_output",
        "delta_pct_output",
        "prev_cache_read",
        "curr_cache_read",
        "delta_cache_read",
        "prev_cache_write",
        "curr_cache_write",
        "delta_cache_write",
        "prev_cost",
        "curr_cost",
        "delta_cost",
    ])?;
    for row in rows {
        w.write_record([
            &row.key,
            &row.presence.to_string(),
            &opt_snap_cell(row.prev.as_ref().map(|s| s.sessions as u64)),
            &opt_snap_cell(row.curr.as_ref().map(|s| s.sessions as u64)),
            &row.delta.sessions.to_string(),
            &opt_snap_cell(row.prev.as_ref().map(|s| s.messages as u64)),
            &opt_snap_cell(row.curr.as_ref().map(|s| s.messages as u64)),
            &row.delta.messages.to_string(),
            &opt_snap_cell(row.prev.as_ref().map(|s| s.tokens.input)),
            &opt_snap_cell(row.curr.as_ref().map(|s| s.tokens.input)),
            &row.delta.tokens.input.to_string(),
            &pct_cell(row.delta.pct.as_ref().and_then(|p| p.input)),
            &opt_snap_cell(row.prev.as_ref().map(|s| s.tokens.output)),
            &opt_snap_cell(row.curr.as_ref().map(|s| s.tokens.output)),
            &row.delta.tokens.output.to_string(),
            &pct_cell(row.delta.pct.as_ref().and_then(|p| p.output)),
            &opt_snap_cell(row.prev.as_ref().map(|s| s.tokens.cache_read)),
            &opt_snap_cell(row.curr.as_ref().map(|s| s.tokens.cache_read)),
            &row.delta.tokens.cache_read.to_string(),
            &opt_snap_cell(row.prev.as_ref().map(|s| s.tokens.cache_write)),
            &opt_snap_cell(row.curr.as_ref().map(|s| s.tokens.cache_write)),
            &row.delta.tokens.cache_write.to_string(),
            &cost_cell(row.prev.as_ref().and_then(|s| s.cost)),
            &cost_cell(row.curr.as_ref().and_then(|s| s.cost)),
            &cost_cell(row.delta.cost),
        ])?;
    }
    w.flush()?;
    Ok(())
}

/// Emit a compact terminal table for the diff.
pub fn render_diff_table(
    rows: &[DiffRow],
    prev_total_sessions: usize,
    curr_total_sessions: usize,
    source_statuses: &[SourceStatus],
) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();

    let sources_line: Vec<String> = source_statuses
        .iter()
        .map(|s| {
            if s.error.is_some() {
                format!("{} ✗", s.name)
            } else {
                format!("{} ●", s.name)
            }
        })
        .collect();

    writeln!(
        out,
        "diff  |  total sessions: prev={} curr={} delta={}",
        prev_total_sessions,
        curr_total_sessions,
        curr_total_sessions as i64 - prev_total_sessions as i64
    )?;
    writeln!(out, "sources: {}", sources_line.join("  "))?;
    writeln!(out)?;

    if rows.is_empty() {
        writeln!(out, "(no groups)")?;
        return Ok(());
    }

    let col_key = col_width(rows, |r| &r.key);
    let col_token = 8usize;

    // Header
    writeln!(out,
        "{:<width$}  | {:>token$} {:>token$} {:>token$} {:>token$}  | {:>token$} {:>token$} {:>token$} {:>token$}  | {:>+token$} | {:>+token$}",
        "key", "p.i", "c.i", "Δi", "Δ%", "p.o", "c.o", "Δo", "Δ%", "sess", "cost",
        width = col_key, token = col_token
    )?;
    writeln!(out,
        "{:<width$}  | {:>token$} {:>token$} {:>token$} {:>token$}  | {:>token$} {:>token$} {:>token$} {:>token$}  | {:>+token$} | {:>+token$}",
        "-".repeat(col_key), "-", "-", "-", "-", "-", "-", "-", "-", "-", "-",
        width = col_key, token = col_token
    )?;

    for row in rows {
        let key_display = if row.presence == Presence::New {
            format!("{} ▲", row.key)
        } else if row.presence == Presence::Removed {
            format!("{} ▼", row.key)
        } else {
            row.key.clone()
        };

        let cell =
            |v: Option<u64>| -> String { v.map(token_disp).unwrap_or_else(|| "—".to_string()) };
        let prev_in = cell(row.prev.as_ref().map(|s| s.tokens.input));
        let curr_in = cell(row.curr.as_ref().map(|s| s.tokens.input));
        let prev_out = cell(row.prev.as_ref().map(|s| s.tokens.output));
        let curr_out = cell(row.curr.as_ref().map(|s| s.tokens.output));

        let delta_in = i64_disp(row.delta.tokens.input);
        let delta_out = i64_disp(row.delta.tokens.output);
        let delta_in_pct = row
            .delta
            .pct
            .as_ref()
            .and_then(|p| p.input)
            .map(|v| format!("{:.1}%", v))
            .unwrap_or_default();
        let delta_out_pct = row
            .delta
            .pct
            .as_ref()
            .and_then(|p| p.output)
            .map(|v| format!("{:.1}%", v))
            .unwrap_or_default();

        let sess_delta = i64_disp(row.delta.sessions);
        let cost_delta = row
            .delta
            .cost
            .map(|c| format!("{:+.6}", c))
            .unwrap_or_else(|| "N/A".to_string());

        writeln!(out,
            "{:<width$}  | {:>token$} {:>token$} {:>token$} {:>token$}  | {:>token$} {:>token$} {:>token$} {:>token$}  | {:>+token$} | {:>+token$}",
            key_display, prev_in, curr_in, delta_in, delta_in_pct,
            prev_out, curr_out, delta_out, delta_out_pct,
            sess_delta, cost_delta,
            width = col_key, token = col_token
        )?;
    }

    Ok(())
}

fn col_width(rows: &[DiffRow], field: impl Fn(&DiffRow) -> &str) -> usize {
    rows.iter()
        .map(|r| field(r).len())
        .max()
        .unwrap_or(0)
        .min(24)
}

fn token_disp(v: u64) -> String {
    format_tokens(v)
}

fn i64_disp(v: i64) -> String {
    if v >= 0 {
        format!("+{}", token_disp(v as u64))
    } else {
        format!("-{}", token_disp((-v) as u64))
    }
}

/// Format a token count with magnitude-appropriate unit (K, M, B).
pub fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let (value, suffix) = if n < 1_000_000 {
        (n as f64 / 1_000.0, "K")
    } else if n < 1_000_000_000 {
        (n as f64 / 1_000_000.0, "M")
    } else {
        (n as f64 / 1_000_000_000.0, "B")
    };
    let text = format!("{:.1}", value);
    let text = text.strip_suffix(".0").unwrap_or(&text);
    if text == "1000" {
        return match suffix {
            "K" => "1M".to_string(),
            "M" => "1B".to_string(),
            _ => "1000B".to_string(),
        };
    }
    format!("{}{}", text, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::AggregateResult;
    use crate::domain::record::{Record, TokenBreakdown};
    use crate::domain::GroupBy;
    use crate::filter::Filter;
    use chrono::Utc;

    fn fake_record(source: &str, project: &str, model: &str, cost: Option<f64>) -> Record {
        Record {
            session_id: format!("ses_{}", source),
            source: source.to_string(),
            project: project.to_string(),
            model: model.to_string(),
            agent: None,
            started_at: Utc::now(),
            ended_at: None,
            tokens: TokenBreakdown::default(),
            message_count: 1,
            cost,
        }
    }

    #[test]
    fn csv_emits_header_and_rows() {
        let records = vec![
            fake_record("claude", "/proj/a", "auto", None),
            fake_record("opencode", "/proj/a", "big-pickle", Some(0.12)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let mut buf = Vec::new();
        OutputRenderer.csv(&agg.groups, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("group_key,source,sessions,messages"));
        assert!(text.contains("claude"));
        assert!(text.contains("opencode"));
    }

    #[test]
    fn csv_cost_blank_for_claude() {
        let records = vec![fake_record("claude", "/p", "auto", None)];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let mut buf = Vec::new();
        OutputRenderer.csv(&agg.groups, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);
        let parts: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(parts[8], "");
    }

    #[test]
    fn json_output_structure() {
        let records = vec![
            fake_record("claude", "/proj/a", "auto", None),
            fake_record("opencode", "/proj/a", "big-pickle", Some(0.12)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let statuses = vec![
            SourceStatus {
                name: "claude".to_string(),
                record_count: 1,
                error: None,
            },
            SourceStatus {
                name: "opencode".to_string(),
                record_count: 1,
                error: None,
            },
        ];
        let mut buf = Vec::new();
        let renderer = OutputRenderer;
        renderer
            .json(&agg.groups, &statuses, "source", &mut buf)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(parsed.get("sources").is_some());
        assert!(parsed.get("groups").is_some());
        assert_eq!(parsed["group_by"], "source");
        let group_keys: Vec<&str> = parsed["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["key"].as_str().unwrap())
            .collect();
        assert!(group_keys.contains(&"claude"));
        assert!(group_keys.contains(&"opencode"));
    }
}

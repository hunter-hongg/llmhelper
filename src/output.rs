use std::collections::BTreeMap;
use std::io::Write;

use serde::Serialize;

use crate::aggregator::{AggregateResult, Group};
use crate::diagnostics::Diagnostics;
use crate::diff::{DiffRow, Presence};
use crate::source::SourceStatus;

#[derive(Clone, Debug, Serialize)]
struct SourceInfo {
    name: String,
    records: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched: Option<usize>,
    status: String,
}

#[derive(Clone, Debug, Serialize)]
struct JsonPayload {
    sources: Vec<SourceInfo>,
    group_by: String,
    groups: Vec<Group>,
    /// Present only when the result is empty or `--explain` was passed, so an
    /// ordinary non-empty run serialises exactly as it always has.
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Diagnostics>,
}

pub struct OutputRenderer;

/// Build the JSON panel entry for one Source.
///
/// `matched` is the loaded count's honest counterpart: how many of that
/// Source's records actually survived the filter. It is only emitted when the
/// funnel is carried (`matched_map` is `Some`), which is exactly the set of
/// runs where the distinction matters — an empty result, or `--explain`.
/// Everywhere else the key is omitted, so the frame stays byte-identical for
/// the ordinary non-empty run that downstream tools parse.
fn source_info(s: &SourceStatus, matched_map: Option<&BTreeMap<String, usize>>) -> SourceInfo {
    SourceInfo {
        name: s.name.clone(),
        records: s.record_count,
        matched: matched_map.map(|m| m.get(&s.name).copied().unwrap_or(0)),
        status: match &s.error {
            None => "ok".to_string(),
            Some(e) => format!("{}", e),
        },
    }
}

impl OutputRenderer {
    /// Render the `usage` / `watch` frame.
    ///
    /// `matched_by_source` is the per-Source breakdown of the *matched* set. It
    /// is only consulted when the funnel is carried, so an ordinary non-empty
    /// run serialises exactly as it always has — see [`source_info`].
    pub fn json<W: Write>(
        &self,
        groups: &[Group],
        source_statuses: &[SourceStatus],
        group_by: &str,
        diagnostics: Option<&Diagnostics>,
        matched_by_source: Option<&BTreeMap<String, usize>>,
        out: &mut W,
    ) -> anyhow::Result<()> {
        // The panel keeps reporting *loaded* counts, but once the funnel is
        // carried the ambiguity it was blamed for is visible right beside it:
        // `records` (loaded) and `matched` (survived the filter) sit on the same
        // row, so a populated panel next to empty `groups` can no longer pose as
        // "there was usage".
        let sources: Vec<SourceInfo> = source_statuses
            .iter()
            .map(|s| source_info(s, diagnostics.and(matched_by_source)))
            .collect();
        let payload = JsonPayload {
            sources,
            group_by: group_by.to_string(),
            groups: groups.to_vec(),
            diagnostics: diagnostics.cloned(),
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
            .map(|s| source_info(s, None))
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

// --- Trend rendering ---

/// The JSON payload for `trend`.
///
/// An **object**, not a bare array, so the bucket list and the funnel can
/// coexist. `buckets` is always dense: an empty trend still emits one entry per
/// bucket, because the number and identity of the rows is itself the answer.
#[derive(Clone, Debug, Serialize)]
struct TrendPayload {
    buckets: Vec<crate::trend::BucketRow>,
    /// Present only when the result is empty or `--explain` was passed — spec
    /// 0020's rule, applied verbatim. An ordinary non-empty trend carries no
    /// key, exactly as `usage --json` does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Diagnostics>,
}

/// Emit the JSON form of a trend.
pub fn render_trend_json<W: Write>(
    rows: &[crate::trend::BucketRow],
    diagnostics: Option<&Diagnostics>,
    out: &mut W,
) -> anyhow::Result<()> {
    let payload = TrendPayload {
        buckets: rows.to_vec(),
        diagnostics: diagnostics.cloned(),
    };
    serde_json::to_writer_pretty(out, &payload)?;
    Ok(())
}

/// Emit the CSV form: a bare header plus one line per bucket, to **stdout**.
///
/// The diagnostics funnel does not belong here — a comment line would corrupt a
/// strict CSV parser — so a caller that has one writes it to stderr (spec
/// 0020's asymmetry). An absent cost is an empty cell rather than `—`, because
/// CSV has no such convention and a blank reads as "no data" to a spreadsheet.
pub fn render_trend_csv<W: Write>(
    rows: &[crate::trend::BucketRow],
    out: &mut W,
) -> anyhow::Result<()> {
    let mut w = csv::Writer::from_writer(out);
    w.write_record([
        "bucket_since",
        "bucket_until",
        "sessions",
        "messages",
        "input",
        "output",
        "cache_read",
        "cache_write",
        "cost",
    ])?;
    for r in rows {
        w.write_record([
            &r.since.to_rfc3339(),
            &r.until.to_rfc3339(),
            &r.sessions.to_string(),
            &r.messages.to_string(),
            &r.tokens.input.to_string(),
            &r.tokens.output.to_string(),
            &r.tokens.cache_read.to_string(),
            &r.tokens.cache_write.to_string(),
            &r.cost.map(|c| format!("{:.6}", c)).unwrap_or_default(),
        ])?;
    }
    w.flush()?;
    Ok(())
}

/// Emit the terminal table for a trend: one row per bucket, oldest first.
///
/// Every bucket is printed, including one with no records, as a row of zeros —
/// "no activity in that bucket" and "no bucket at all" must not render alike.
/// The header names the **resolved span** (first bucket's start → last bucket's
/// end) rather than echoing the flags, so a clamped tail is visible.
pub fn render_trend_table(
    rows: &[crate::trend::BucketRow],
    bucket_days: u32,
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
    writeln!(out, "sources: {}", sources_line.join("  "))?;

    match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => writeln!(
            out,
            "trend  |  {} → {}",
            first.since.with_timezone(&chrono::Local).format("%Y-%m-%d"),
            last.until.with_timezone(&chrono::Local).format("%Y-%m-%d"),
        )?,
        _ => writeln!(out, "trend  |  (no buckets)")?,
    }
    writeln!(out)?;

    if rows.is_empty() {
        writeln!(out, "(no buckets)")?;
        return Ok(());
    }

    // The label column widens for multi-day buckets, whose labels are spans
    // (`2026-08-23 → 08-30`, 18 chars) rather than single dates.
    let label_width = if bucket_days <= 1 { 10usize } else { 18usize };
    let num_width = 8usize;
    let tok_width = 8usize;

    writeln!(
        out,
        "{:<label$}  {:>num$}  {:>num$}  {:>tok$}  {:>tok$}  {:>tok$}  {:>tok$}  {:>tok$}",
        "bucket",
        "sessions",
        "messages",
        "input",
        "output",
        "cache_read",
        "cache_write",
        "cost",
        label = label_width,
        num = num_width,
        tok = tok_width
    )?;
    let dashes = |n: usize| "-".repeat(n);
    writeln!(
        out,
        "{:<label$}  {:>num$}  {:>num$}  {:>tok$}  {:>tok$}  {:>tok$}  {:>tok$}  {:>tok$}",
        dashes(label_width),
        dashes(num_width),
        dashes(num_width),
        dashes(tok_width),
        dashes(tok_width),
        dashes(tok_width),
        dashes(tok_width),
        dashes(tok_width),
        label = label_width,
        num = num_width,
        tok = tok_width
    )?;

    for r in rows {
        // Cost is `—` when no source in the bucket recorded one, never `0.000000`
        // — a blank would be read as "spent nothing".
        let cost = r
            .cost
            .map(|c| format!("{:.6}", c))
            .unwrap_or_else(|| "—".to_string());
        writeln!(
            out,
            "{:<label$}  {:>num$}  {:>num$}  {:>tok$}  {:>tok$}  {:>tok$}  {:>tok$}  {:>tok$}",
            crate::trend::bucket_label(
                &crate::trend::Bucket {
                    since: r.since,
                    until: r.until
                },
                bucket_days,
            ),
            r.sessions,
            r.messages,
            format_tokens(r.tokens.input),
            format_tokens(r.tokens.output),
            format_tokens(r.tokens.cache_read),
            format_tokens(r.tokens.cache_write),
            cost,
            label = label_width,
            num = num_width,
            tok = tok_width
        )?;
    }
    Ok(())
}

// --- Compare rendering ---

/// A ranked row's JSON shape. Shares are nested under `token_shares` so the
/// per-field breakdown reads as a unit, mirroring `tokens`.
#[derive(Clone, Debug, Serialize)]
struct CompareRowPayload<'a> {
    rank: usize,
    key: &'a str,
    is_others: bool,
    sessions: usize,
    messages: usize,
    tokens: &'a crate::domain::record::TokenBreakdown,
    cost: Option<f64>,
    /// The sort metric's share of the whole, or `null` when undefined.
    share_pct: Option<f64>,
    token_shares: CompareTokenShares,
    /// The budget verdict for this row's Source, `"over"`/`"under"`/
    /// `"not_measured"`. Absent (not `null`) when no budget bears on the row —
    /// including every `(others)` row and any run with no budgets at all (spec
    /// 0024), so a no-budget payload is byte-identical to before.
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_state: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize)]
struct CompareTokenShares {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

/// The JSON payload for `compare`.
///
/// An **object**, not a bare array, so the ranking and the funnel can coexist.
#[derive(Clone, Debug, Serialize)]
struct ComparePayload<'a> {
    group_by: &'a str,
    sort_by: &'a str,
    grand_tokens: u64,
    rows: Vec<CompareRowPayload<'a>>,
    /// The budget summary, present only when budgets were configured (spec
    /// 0024), so a no-budget payload is byte-identical to before.
    #[serde(skip_serializing_if = "Option::is_none")]
    budgets: Option<CompareBudgets>,
    /// Present only when the result is empty or `--explain` was passed — spec
    /// 0020's rule, applied verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Diagnostics>,
}

/// The `budgets` object in `compare`'s JSON, mirroring `BudgetSummary`.
#[derive(Clone, Copy, Debug, Serialize)]
struct CompareBudgets {
    over: usize,
    under: usize,
    not_measured: usize,
}

/// Emit the JSON form of a ranked comparison.
///
/// `grand_tokens` is the aggregate's own four-field total, threaded through so
/// the payload's denominator matches the one the shares were computed against.
pub fn render_compare_json<W: Write>(
    rows: &[crate::compare::RankedGroup],
    group_by: &str,
    sort_by: &str,
    grand_tokens: u64,
    budgets: Option<&crate::compare::BudgetSummary>,
    diagnostics: Option<&Diagnostics>,
    out: &mut W,
) -> anyhow::Result<()> {
    let rows = rows
        .iter()
        .map(|r| CompareRowPayload {
            rank: r.rank,
            key: &r.key,
            is_others: r.is_others,
            sessions: r.sessions,
            messages: r.messages,
            tokens: &r.tokens,
            cost: r.cost,
            share_pct: r.share_pct,
            token_shares: CompareTokenShares {
                input: r.input_share_pct,
                output: r.output_share_pct,
                cache_read: r.cache_read_share_pct,
                cache_write: r.cache_write_share_pct,
            },
            budget_state: r.budget_state.as_ref().map(|s| s.json_label()),
        })
        .collect();
    let payload = ComparePayload {
        group_by,
        sort_by,
        grand_tokens,
        rows,
        budgets: budgets.map(|b| CompareBudgets {
            over: b.over,
            under: b.under,
            not_measured: b.not_measured,
        }),
        diagnostics: diagnostics.cloned(),
    };
    serde_json::to_writer_pretty(out, &payload)?;
    Ok(())
}

/// Emit the CSV form: a bare header plus one line per ranked group, to
/// **stdout**. An absent cost or share is an empty cell, not `—`.
///
/// A `budget_state` column is appended **only** when `with_budgets` is true, so
/// a run with no budgets keeps the original 15-column header byte-for-byte.
pub fn render_compare_csv<W: Write>(
    rows: &[crate::compare::RankedGroup],
    with_budgets: bool,
    out: &mut W,
) -> anyhow::Result<()> {
    let mut w = csv::Writer::from_writer(out);
    let mut header = vec![
        "rank",
        "key",
        "is_others",
        "sessions",
        "messages",
        "input",
        "output",
        "cache_read",
        "cache_write",
        "cost",
        "share_pct",
        "input_share_pct",
        "output_share_pct",
        "cache_read_share_pct",
        "cache_write_share_pct",
    ];
    if with_budgets {
        header.push("budget_state");
    }
    w.write_record(&header)?;
    let num = |v: Option<f64>| v.map(|x| format!("{x}")).unwrap_or_default();
    for r in rows {
        let mut record = vec![
            r.rank.to_string(),
            r.key.clone(),
            r.is_others.to_string(),
            r.sessions.to_string(),
            r.messages.to_string(),
            r.tokens.input.to_string(),
            r.tokens.output.to_string(),
            r.tokens.cache_read.to_string(),
            r.tokens.cache_write.to_string(),
            r.cost.map(|c| format!("{:.6}", c)).unwrap_or_default(),
            num(r.share_pct),
            num(r.input_share_pct),
            num(r.output_share_pct),
            num(r.cache_read_share_pct),
            num(r.cache_write_share_pct),
        ];
        if with_budgets {
            // An absent state is an empty cell, never `under` or `0`.
            record.push(
                r.budget_state
                    .as_ref()
                    .map(|s| s.json_label().to_string())
                    .unwrap_or_default(),
            );
        }
        w.write_record(&record)?;
    }
    w.flush()?;
    Ok(())
}

/// Emit the terminal table for a ranked comparison: one row per group, largest
/// first, with each group's share of the whole.
///
/// The `(others)` fold, when present, is printed last and its key makes it
/// unmistakable for a real group. An empty result prints the header and no rows
/// — a valid empty leaderboard — with `--explain` explaining why.
pub fn render_compare_table(
    rows: &[crate::compare::RankedGroup],
    group_by: &str,
    sort_by: &str,
    budgets: Option<&crate::compare::BudgetSummary>,
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
    writeln!(out, "sources: {}", sources_line.join("  "))?;
    if let Some(b) = budgets {
        writeln!(
            out,
            "compare  |  group: {}   sort: {}   rows: {}   {}",
            group_by,
            sort_by,
            rows.len(),
            b.label()
        )?;
    } else {
        writeln!(
            out,
            "compare  |  group: {}   sort: {}   rows: {}",
            group_by,
            sort_by,
            rows.len()
        )?;
    }
    writeln!(out)?;

    let key_width = rows
        .iter()
        .map(|r| r.key.chars().count())
        .max()
        .unwrap_or(6)
        .max(6);
    let rank_width = 4usize;
    let num_width = 8usize;
    let tok_width = 8usize;
    let share_width = 7usize;

    // The status gutter is a fixed two-cell prefix, present only when budgets
    // were configured, so a no-budget table is byte-identical to before and
    // existing column-width assumptions hold (spec 0024).
    let gutter = if budgets.is_some() { "⚠ " } else { "" };
    writeln!(
        out,
        "{}{:>rank$}  {:<key$}  {:>num$}  {:>num$}  {:>tok$}  {:>share$}  {:>tok$}",
        if budgets.is_some() { "  " } else { "" },
        "rank",
        "group",
        "sessions",
        "messages",
        "tokens",
        "share",
        "cost",
        rank = rank_width,
        key = key_width,
        num = num_width,
        tok = tok_width,
        share = share_width
    )?;
    for r in rows {
        // An absent cost or share is `—`, never `0`, so "no data" and "zero" do
        // not render alike.
        let cost = r
            .cost
            .map(|c| format!("{:.6}", c))
            .unwrap_or_else(|| "—".to_string());
        let share = r
            .share_pct
            .map(|p| format!("{p:.1}%"))
            .unwrap_or_else(|| "—".to_string());
        // Only an `over` verdict earns the glyph: `under` and `not_measured`
        // render an empty gutter, because absence of `⚠` means "not over",
        // not "no budget" (the header states the budget situation).
        let flag = if r.budget_state == Some(crate::budget::BudgetState::Over) {
            gutter
        } else if budgets.is_some() {
            "  "
        } else {
            ""
        };
        writeln!(
            out,
            "{}{:>rank$}  {:<key$}  {:>num$}  {:>num$}  {:>tok$}  {:>share$}  {:>tok$}",
            flag,
            r.rank,
            r.key,
            r.sessions,
            r.messages,
            format_tokens(r.tokens.total()),
            share,
            cost,
            rank = rank_width,
            key = key_width,
            num = num_width,
            tok = tok_width,
            share = share_width
        )?;
    }
    Ok(())
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
            .json(&agg.groups, &statuses, "source", None, None, &mut buf)
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

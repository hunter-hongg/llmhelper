use crate::aggregator::AggregateResult;
use crate::output::format_tokens;
use crate::source::SourceStatus;

/// Descriptive metadata rendered into the report header. Built by the caller
/// so `render_report` stays a pure function of its inputs.
#[derive(Clone, Debug)]
pub struct ReportMeta {
    /// When the report was generated (UTC).
    pub generated_at: chrono::DateTime<chrono::Utc>,
    /// Human description of the time window, e.g. "last 7d" or "all time".
    pub window: String,
    /// Label of the grouping dimension, e.g. "source".
    pub group_by: String,
    /// Applied non-temporal filters, echoed as (label, value) pairs.
    pub filters: Vec<(String, String)>,
    /// Custom document title. Falls back to "llmhelper report" when absent.
    pub title: Option<String>,
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn cost_cell(cost: Option<f64>) -> String {
    cost.map(|c| format!("{:.6}", c)).unwrap_or_else(|| "—".to_string())
}

/// Render the complete Markdown report as a string. Pure: no I/O.
pub fn render_report(
    agg: &AggregateResult,
    meta: &ReportMeta,
    statuses: &[SourceStatus],
    top: Option<usize>,
) -> String {
    let mut out = String::new();

    // --- Header ---
    let title = meta.title.as_deref().unwrap_or("llmhelper report");
    out.push_str(&format!("# {}\n\n", md_cell(title)));
    out.push_str(&format!(
        "- generated: {}\n",
        meta.generated_at.format("%Y-%m-%dT%H:%M:%SZ")
    ));
    out.push_str(&format!("- window: {}\n", md_cell(&meta.window)));
    if meta.filters.is_empty() {
        out.push_str("- filters: (none)\n");
    } else {
        let joined = meta
            .filters
            .iter()
            .map(|(k, v)| format!("{}={}", k, md_cell(v)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("- filters: {}\n", joined));
    }
    out.push('\n');

    // --- Totals ---
    out.push_str("## Totals\n\n");
    out.push_str("| sessions | messages | input | output | cache read | cache write |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    out.push_str(&format!(
        "| {} | {} | {} | {} | {} | {} |\n\n",
        agg.grand_sessions,
        agg.grand_messages,
        format_tokens(agg.grand_totals.input),
        format_tokens(agg.grand_totals.output),
        format_tokens(agg.grand_totals.cache_read),
        format_tokens(agg.grand_totals.cache_write),
    ));

    // --- Cost by source (per-source sums only; never cross-source) ---
    let mut cost_rows: Vec<(String, f64)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for g in &agg.groups {
        if let Some(c) = g.cost {
            if seen.insert(g.source.clone()) {
                cost_rows.push((g.source.clone(), c));
            } else if let Some(entry) = cost_rows.iter_mut().find(|(s, _)| *s == g.source) {
                entry.1 += c;
            }
        }
    }
    cost_rows.sort_by(|a, b| a.0.cmp(&b.0));
    if cost_rows.is_empty() {
        out.push_str("## Cost by source\n\n(no source records cost)\n\n");
    } else {
        out.push_str("## Cost by source\n\n");
        out.push_str("| source | cost |\n");
        out.push_str("|---|---|\n");
        for (source, c) in &cost_rows {
            out.push_str(&format!("| {} | {:.6} |\n", md_cell(source), c));
        }
        out.push('\n');
    }

    // --- Usage groups ---
    out.push_str(&format!("## Usage by {}\n\n", md_cell(&meta.group_by)));

    if agg.groups.is_empty() {
        out.push_str("no sessions matched the current filters\n\n");
    } else {
        let mut sorted: Vec<&crate::aggregator::Group> = agg.groups.iter().collect();
        sorted.sort_by(|a, b| {
            b.tokens
                .input
                .cmp(&a.tokens.input)
                .then_with(|| a.key.cmp(&b.key))
        });

        out.push_str(
            "| key | sessions | messages | input | output | cache read | cache write | cost |\n",
        );
        out.push_str("|---|---|---|---|---|---|---|---|\n");

        let shown = top.map(|n| n.min(sorted.len())).unwrap_or(sorted.len());
        for g in &sorted[..shown] {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                md_cell(&g.key),
                g.sessions,
                g.messages,
                format_tokens(g.tokens.input),
                format_tokens(g.tokens.output),
                format_tokens(g.tokens.cache_read),
                format_tokens(g.tokens.cache_write),
                cost_cell(g.cost),
            ));
        }
        if shown < sorted.len() {
            let omitted = &sorted[shown..];
            let k = omitted.len();
            let hidden_input: u64 = omitted.iter().map(|g| g.tokens.input).sum();
            out.push_str(&format!(
                "_(+ {} more — {} input tokens)_\n",
                k,
                format_tokens(hidden_input)
            ));
        }
        out.push('\n');
    }

    // --- Sources appendix ---
    out.push_str("## Sources\n\n");
    out.push_str("| source | records | status |\n");
    out.push_str("|---|---|---|\n");
    for s in statuses {
        let status = match &s.error {
            None => "ok".to_string(),
            Some(e) => format!("{}", e),
        };
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            md_cell(&s.name),
            s.record_count,
            md_cell(&status)
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::Group;
    use crate::domain::record::TokenBreakdown;
    use crate::source::SourceError;
    use chrono::Utc;

    fn group(key: &str, source: &str, input: u64, cost: Option<f64>) -> Group {
        Group {
            key: key.to_string(),
            source: source.to_string(),
            sessions: 2,
            messages: 4,
            tokens: TokenBreakdown {
                input,
                output: input / 2,
                cache_read: 1,
                cache_write: 1,
            },
            cost,
        }
    }

    fn meta() -> ReportMeta {
        ReportMeta {
            generated_at: Utc::now(),
            window: "last 7d".to_string(),
            group_by: "source".to_string(),
            filters: vec![("project".to_string(), "/proj/a".to_string())],
            title: None,
        }
    }

    fn agg_with(groups: Vec<Group>) -> AggregateResult {
        let grand_totals = groups
            .iter()
            .map(|g| g.tokens.clone())
            .fold(TokenBreakdown::default(), |mut acc, t| {
                acc.add(&t);
                acc
            });
        AggregateResult {
            grand_totals,
            grand_messages: groups.iter().map(|g| g.messages).sum(),
            grand_sessions: groups.iter().map(|g| g.sessions).sum(),
            groups,
        }
    }

    fn ok_status(name: &str, n: usize) -> SourceStatus {
        SourceStatus {
            name: name.to_string(),
            record_count: n,
            error: None,
        }
    }

    #[test]
    fn header_echoes_window_and_filters() {
        let report = render_report(&agg_with(vec![]), &meta(), &[ok_status("claude", 0)], None);
        assert!(report.starts_with("# llmhelper report\n"));
        assert!(report.contains("window: last 7d"));
        assert!(report.contains("project=/proj/a"));
        assert!(report.contains("generated:"));
    }

    #[test]
    fn custom_title_replaces_default_heading() {
        let m = ReportMeta {
            title: Some("Team weekly LLM usage".to_string()),
            ..meta()
        };
        let report = render_report(&agg_with(vec![]), &m, &[], None);
        assert!(report.starts_with("# Team weekly LLM usage\n"));
    }

    #[test]
    fn title_pipe_is_escaped() {
        let m = ReportMeta {
            title: Some("Team|Weekly".to_string()),
            ..meta()
        };
        let report = render_report(&agg_with(vec![]), &m, &[], None);
        assert!(report.starts_with("# Team\\|Weekly\n"));
    }

    #[test]
    fn no_filters_renders_none() {
        let m = ReportMeta {
            generated_at: Utc::now(),
            window: "all time".to_string(),
            group_by: "project".to_string(),
            filters: vec![],
            title: None,
        };
        let report = render_report(&agg_with(vec![]), &m, &[], None);
        assert!(report.contains("filters: (none)"));
        assert!(report.contains("window: all time"));
        assert!(report.contains("## Usage by project"));
    }

    #[test]
    fn totals_row_matches_grand_values() {
        let agg = agg_with(vec![
            group("claude", "claude", 1_000, None),
            group("opencode", "opencode", 250, Some(0.5)),
        ]);
        let report = render_report(&agg, &meta(), &[], None);
        assert!(report.contains("## Totals"));
        // grand: sessions 2+2=4, messages 4+4=8, input 1250 -> 1.2K,
        // output 625, cache_read 2, cache_write 2.
        assert!(report.contains("| 4 | 8 | 1.2K | 625 | 2 | 2 |"));
    }

    #[test]
    fn cost_by_source_is_per_source_not_cross_source() {
        let agg = agg_with(vec![
            group("claude", "claude", 100, None),
            group("opencode", "opencode", 200, Some(0.5)),
            group("big-pickle", "opencode", 300, Some(1.0)),
            group("omp", "omp", 50, Some(0.2)),
        ]);
        let report = render_report(&agg, &meta(), &[], None);
        assert!(report.contains("## Cost by source"));
        assert!(report.contains("| opencode | 1.500000 |"));
        assert!(report.contains("| omp | 0.200000 |"));
        // claude has no cost anywhere; no cross-source total (1.7) may appear.
        assert!(!report.contains("1.700000"));
    }

    #[test]
    fn cost_by_source_empty_when_none_record_cost() {
        let agg = agg_with(vec![group("claude", "claude", 100, None)]);
        let report = render_report(&agg, &meta(), &[], None);
        assert!(report.contains("(no source records cost)"));
    }

    #[test]
    fn groups_sorted_by_input_desc() {
        let agg = agg_with(vec![
            group("small", "claude", 10, None),
            group("big", "claude", 10_000, None),
            group("mid", "claude", 500, None),
        ]);
        let report = render_report(&agg, &meta(), &[], None);
        let big = report.find("| big ").unwrap();
        let mid = report.find("| mid ").unwrap();
        let small = report.find("| small ").unwrap();
        assert!(big < mid && mid < small);
    }

    #[test]
    fn top_truncates_and_reports_hidden_input() {
        let agg = agg_with(vec![
            group("a", "claude", 100, None),
            group("b", "claude", 200, None),
            group("c", "claude", 700, None),
        ]);
        let report = render_report(&agg, &meta(), &[], Some(1));
        assert!(report.contains("| c |"));
        assert!(!report.contains("| a |"));
        assert!(!report.contains("| b |"));
        assert!(report.contains("(+ 2 more"));
        // a + b = 300 input tokens collapsed.
        assert!(report.contains("300 input tokens"));
    }

    #[test]
    fn unavailable_cost_renders_em_dash_not_zero() {
        let agg = agg_with(vec![group("claude", "claude", 100, None)]);
        let report = render_report(&agg, &meta(), &[], None);
        let row = report
            .lines()
            .find(|l| l.starts_with("| claude "))
            .unwrap();
        assert!(row.ends_with("| — |"));
        assert!(!row.contains("| 0.000000 |"));
    }

    #[test]
    fn empty_aggregate_renders_no_match_note() {
        let report = render_report(&agg_with(vec![]), &meta(), &[], None);
        assert!(report.contains("no sessions matched the current filters"));
        assert!(report.contains("## Sources"));
    }

    #[test]
    fn source_errors_surface_in_appendix() {
        let statuses = vec![SourceStatus {
            name: "opencode".to_string(),
            record_count: 0,
            error: Some(SourceError::Absent("/missing/db".to_string())),
        }];
        let report = render_report(&agg_with(vec![]), &meta(), &statuses, None);
        assert!(report.contains("| opencode | 0 | absent: /missing/db |"));
    }

    #[test]
    fn pipe_in_project_key_is_escaped() {
        let agg = agg_with(vec![group("a|b", "claude", 100, None)]);
        let report = render_report(&agg, &meta(), &[], None);
        assert!(report.contains("| a\\|b |"));
    }
}

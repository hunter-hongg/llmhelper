use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use super::watch_app::{WatchApp, WatchTuiState};
use crate::aggregator::Group;
use crate::source::SourceStatus;

// ---------------------------------------------------------------------------
// Palette — the same calm dark theme as the `usage` and `diff` TUIs, so the
// three views read as one app. Color semantics are kept identical to the
// others: RED is the "crossed the line" color everywhere (budget flags on the
// usage page, negative deltas on the diff page), so it means the same thing
// here.
// ---------------------------------------------------------------------------
const BG: Color = Color::Rgb(15, 17, 23);
const SURFACE: Color = Color::Rgb(20, 23, 33);
const BORDER: Color = Color::Rgb(42, 47, 58);
const TITLE: Color = Color::Rgb(139, 149, 168);
const TEXT: Color = Color::Rgb(201, 209, 217);
const MUTED: Color = Color::Rgb(110, 118, 129);
const ACCENT: Color = Color::Rgb(86, 212, 221);
const GREEN: Color = Color::Rgb(126, 231, 135);
const BLUE: Color = Color::Rgb(121, 192, 255);
const YELLOW: Color = Color::Rgb(227, 179, 65);
const RED: Color = Color::Rgb(255, 123, 114);

fn source_color(name: &str) -> Color {
    match name {
        "claude" => Color::Rgb(255, 138, 76),
        "opencode" => BLUE,
        "omp" => GREEN,
        "kilo" => YELLOW,
        "mixed" => MUTED,
        _ => MUTED,
    }
}

fn panel(block_title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG))
        .title(Line::from(Span::styled(
            format!(" {} ", block_title),
            Style::default().fg(TITLE),
        )))
}

fn clock(t: &DateTime<Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

/// The `—` used for every value that was never measured, so a blank is never
/// confusable with a measured zero.
const UNKNOWN: &str = "—";

pub fn render(frame: &mut Frame, state: &mut WatchTuiState, now: DateTime<Utc>) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);

    let chunks = Layout::default()
        .constraints([
            Constraint::Length(6), // header: identity, freshness, window, budget
            Constraint::Length(3), // sources
            Constraint::Min(8),    // group table
            Constraint::Length(3), // key hints
        ])
        .split(area);

    frame.render_widget(render_header(state, now), chunks[0]);
    frame.render_widget(render_sources(&state.app.source_statuses), chunks[1]);
    frame.render_widget(render_table(state), chunks[2]);
    frame.render_widget(render_footer(), chunks[3]);
}

/// Header content: identity and freshness on the first line, what the frame
/// covers on the second, and the budget state on the third — but only when
/// there is a budget to report. An absent line is used rather than an empty
/// one so a run without budgets carries no trace of the feature.
fn header_lines(state: &WatchTuiState, now: DateTime<Utc>) -> Vec<Line<'static>> {
    let app = &state.app;

    let mut first = vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  watch", Style::default().fg(TITLE)),
    ];
    if let Some(updated) = app.loaded_at {
        first.push(Span::styled(
            format!("   updated {}", clock(&updated)),
            Style::default().fg(MUTED),
        ));
    } else {
        first.push(Span::styled(
            format!("   updated {}", UNKNOWN),
            Style::default().fg(MUTED),
        ));
    }
    let countdown = app.countdown_text(now);
    if !countdown.is_empty() {
        let style = if countdown.starts_with("reloading") {
            Style::default().fg(MUTED)
        } else {
            Style::default().fg(ACCENT)
        };
        first.push(Span::styled(format!("   {}", countdown), style));
    }

    let mut second = Vec::new();
    match app.window_span_text() {
        Some(span) => {
            let (from, to) = match (app.prev_loaded_at, app.loaded_at) {
                (Some(from), Some(to)) => (clock(&from), clock(&to)),
                _ => (UNKNOWN.to_string(), UNKNOWN.to_string()),
            };
            second.push(Span::styled(
                format!("active {} → {} ({})", from, to, span),
                Style::default().fg(TEXT),
            ));
        }
        // No previous frame yet: say so rather than claiming a one-frame span
        // of zero seconds.
        None => second.push(Span::styled(
            "active — (first frame)",
            Style::default().fg(MUTED),
        )),
    }
    if let Some(label) = app.window_label() {
        second.push(Span::styled(
            format!("   window: {}", label),
            Style::default().fg(MUTED),
        ));
    }

    let mut lines = vec![Line::from(first), Line::from(second)];
    if let Some(indicator) = app.budget_indicator() {
        let over = indicator.contains("over");
        lines.push(Line::from(Span::styled(
            indicator,
            Style::default()
                .fg(if over { RED } else { MUTED })
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines
}

fn render_header(state: &WatchTuiState, now: DateTime<Utc>) -> Paragraph<'static> {
    Paragraph::new(header_lines(state, now)).block(panel("llmhelper watch"))
}

fn render_sources(statuses: &[SourceStatus]) -> Paragraph<'static> {
    let mut spans: Vec<Span> = Vec::new();
    if statuses.is_empty() {
        spans.push(Span::styled("no sources", Style::default().fg(MUTED)));
    }
    for s in statuses {
        let ok = s.error.is_none();
        let color = if ok { source_color(&s.name) } else { RED };
        let mark = if ok { "●" } else { "✗" };
        spans.push(Span::styled(
            format!(" {} ", mark),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            s.name.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        if let Some(err) = &s.error {
            spans.push(Span::styled(format!(" {}", err), Style::default().fg(RED)));
        } else {
            spans.push(Span::styled(
                format!(" {} records loaded", s.record_count),
                Style::default().fg(MUTED),
            ));
        }
        spans.push(Span::raw("    "));
    }
    Paragraph::new(Line::from(spans)).block(panel("sources"))
}

fn table_header(group_by: crate::domain::group::GroupBy) -> Vec<String> {
    vec![
        group_by.label().to_string(),
        "sessions".to_string(),
        "messages".to_string(),
        "tokens".to_string(),
        "Δtokens".to_string(),
        "cost".to_string(),
        "Δcost".to_string(),
    ]
}

/// One row per group, annotated with what changed since the previous read.
///
/// Deltas are `—` whenever they were not observed (the first frame, or a cost
/// the sources do not record) rather than a zero that would read as "nothing
/// moved".
fn row_cells(app: &WatchApp, g: &Group) -> Vec<(String, Style)> {
    let over = app.source_is_over_budget(&g.source);
    let key_style = if over {
        Style::default().fg(RED).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    let key = if over {
        format!("⚠ {}", g.key)
    } else {
        g.key.clone()
    };

    let token_delta = match app.token_delta(&g.key) {
        Some(d) => (signed_tokens(d), delta_style_i(d)),
        None => (UNKNOWN.to_string(), Style::default().fg(MUTED)),
    };
    let cost_delta = match app.cost_delta(&g.key) {
        Some(d) => (format!("{:+.6}", d), delta_style_f(d)),
        None => (UNKNOWN.to_string(), Style::default().fg(MUTED)),
    };
    let cost = match g.cost {
        Some(c) => (format!("{:.6}", c), Style::default().fg(TEXT)),
        None => (UNKNOWN.to_string(), Style::default().fg(MUTED)),
    };

    vec![
        (key, key_style),
        (g.sessions.to_string(), Style::default().fg(TEXT)),
        (g.messages.to_string(), Style::default().fg(TEXT)),
        (
            crate::output::format_tokens(
                g.tokens.input + g.tokens.output + g.tokens.cache_read + g.tokens.cache_write,
            ),
            Style::default().fg(TEXT),
        ),
        token_delta,
        cost,
        cost_delta,
    ]
}

fn render_table(state: &WatchTuiState) -> Table<'static> {
    let app = &state.app;
    let groups: &[Group] = app
        .result
        .as_ref()
        .map(|r| r.groups.as_slice())
        .unwrap_or(&[]);

    let rows: Vec<Row> = groups
        .iter()
        .map(|g| {
            Row::new(
                row_cells(app, g)
                    .into_iter()
                    .map(|(text, style)| Cell::from(text).style(style)),
            )
        })
        .collect();

    let header = Row::new(table_header(app.group_by))
        .style(Style::default().fg(TITLE).add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Min(16),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(11),
    ];

    // No selection: a monitor's table is scrolled, never highlighted. The
    // `TableState` handed to `Table::render` carries only the offset that
    // survives reloads.
    Table::new(rows, widths)
        .header(header)
        .block(panel("usage"))
}

fn signed_tokens(d: i64) -> String {
    let magnitude = crate::output::format_tokens(d.unsigned_abs());
    if d > 0 {
        format!("+{}", magnitude)
    } else if d < 0 {
        format!("-{}", magnitude)
    } else {
        "0".to_string()
    }
}

fn delta_style_i(d: i64) -> Style {
    if d > 0 {
        Style::default().fg(GREEN)
    } else if d < 0 {
        Style::default().fg(RED)
    } else {
        Style::default().fg(MUTED)
    }
}

fn delta_style_f(d: f64) -> Style {
    if d > 0.0 {
        Style::default().fg(GREEN)
    } else if d < 0.0 {
        Style::default().fg(RED)
    } else {
        Style::default().fg(MUTED)
    }
}

fn footer_line() -> Line<'static> {
    let keys: &[(&str, &str)] = &[("r", "reload"), ("q", "quit")];
    let mut spans = vec![Span::styled(
        "live monitor  ",
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    )];
    for (k, label) in keys {
        spans.push(Span::styled(
            format!("  {} ", k),
            Style::default()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{}   ", label),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn render_footer() -> Paragraph<'static> {
    Paragraph::new(footer_line()).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::AggregateResult;
    use crate::budget::{Budget, BudgetState, BudgetStatus, BudgetWindow, EvaluatedBudget};
    use crate::domain::group::GroupBy;
    use crate::domain::record::TokenBreakdown;
    use crate::domain::window::WindowMode;
    use crate::tui::watch_app::{WatchApp, WatchTuiState};

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn group(key: &str, tokens: u64, cost: Option<f64>) -> Group {
        Group {
            key: key.to_string(),
            source: key.to_string(),
            sessions: 1,
            messages: 2,
            tokens: TokenBreakdown {
                input: tokens,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            cost,
        }
    }

    fn agg(groups: Vec<Group>) -> AggregateResult {
        AggregateResult {
            grand_totals: TokenBreakdown::default(),
            grand_messages: 0,
            grand_sessions: 0,
            groups,
        }
    }

    fn state_with(frames: &[Vec<Group>]) -> WatchTuiState {
        let mut state = WatchTuiState {
            app: WatchApp {
                interval_secs: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        for (i, groups) in frames.iter().enumerate() {
            state.apply_data(
                Some(agg(groups.clone())),
                Vec::new(),
                Vec::new(),
                at(i as i64 * 5),
            );
        }
        state
    }

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn evaluated(source: &str, state: BudgetState) -> EvaluatedBudget {
        EvaluatedBudget {
            status: BudgetStatus {
                budget: Budget {
                    name: format!("{}-daily", source),
                    source: source.to_string(),
                    window: BudgetWindow::parse("1d").unwrap(),
                    max_cost: 5.0,
                },
                spend: Some(6.0),
                state,
            },
            measurement: crate::budget::Measurement {
                lower_bound: None,
                clipped_by: None,
            },
        }
    }

    #[test]
    fn header_reports_identity_freshness_and_countdown() {
        let state = state_with(&[vec![group("opencode", 10, None)]]);
        let out = plain(&header_lines(&state, at(0)));
        assert!(out.contains("llmhelper"), "{}", out);
        assert!(out.contains("watch"), "{}", out);
        assert!(out.contains("updated "), "{}", out);
        assert!(out.contains("next in 5s"), "{}", out);
    }

    #[test]
    fn footer_advertises_only_reload_and_quit() {
        let out = footer_line()
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>();
        assert!(out.contains("r "), "{}", out);
        assert!(out.contains("reload"), "{}", out);
        assert!(out.contains("q "), "{}", out);
        assert!(out.contains("quit"), "{}", out);
        // A monitor has no selection or detail view to advertise.
        assert!(!out.contains("select"), "{}", out);
        assert!(!out.contains("Tab"), "{}", out);
    }

    #[test]
    fn first_frame_says_so_instead_of_claiming_a_zero_span() {
        let state = state_with(&[vec![group("opencode", 10, None)]]);
        let out = plain(&header_lines(&state, at(0)));
        assert!(out.contains("first frame"), "{}", out);
        assert!(!out.contains("(0s)"), "{}", out);
    }

    #[test]
    fn later_frames_describe_the_elapsed_span() {
        let state = state_with(&[
            vec![group("opencode", 10, None)],
            vec![group("opencode", 20, None)],
        ]);
        let out = plain(&header_lines(&state, at(5)));
        assert!(out.contains("(5s)"), "{}", out);
    }

    #[test]
    fn header_names_a_rolling_and_a_calendar_window() {
        let mut state = state_with(&[vec![group("opencode", 10, None)]]);
        state.app.mode = Some(WindowMode::Calendar { days: 1 });
        assert!(plain(&header_lines(&state, at(0))).contains("window: last 1d (calendar)"));

        state.app.mode = Some(WindowMode::Rolling {
            last: chrono::Duration::days(7),
        });
        assert!(plain(&header_lines(&state, at(0))).contains("window: last 7d"));
    }

    #[test]
    fn budget_line_is_omitted_entirely_when_nothing_is_configured() {
        let state = state_with(&[vec![group("opencode", 10, None)]]);
        let lines = header_lines(&state, at(0));
        assert_eq!(lines.len(), 2, "no third line without budgets");
        assert!(!plain(&lines).contains("budget"), "{}", plain(&lines));
    }

    #[test]
    fn budget_line_is_rendered_and_red_when_over() {
        let mut state = state_with(&[vec![group("opencode", 10, Some(6.0))]]);
        state.app.budgets = vec![evaluated("opencode", BudgetState::Over)];
        let lines = header_lines(&state, at(0));
        assert_eq!(lines.len(), 3);
        assert!(plain(&lines).contains("budget: 1 over"));
        assert_eq!(lines[2].spans[0].style.fg, Some(RED));
    }

    #[test]
    fn budget_line_is_muted_when_not_over() {
        let mut state = state_with(&[vec![group("opencode", 10, Some(1.0))]]);
        state.app.budgets = vec![evaluated("opencode", BudgetState::Under)];
        let lines = header_lines(&state, at(0));
        assert!(plain(&lines).contains("budget: ok"));
        assert_eq!(lines[2].spans[0].style.fg, Some(MUTED));
    }

    #[test]
    fn first_frame_rows_show_dashes_not_zeros() {
        let state = state_with(&[vec![group("opencode", 10, None)]]);
        let rendered = render_rows(&state);
        assert_eq!(rendered[0][4], UNKNOWN, "Δtokens on the first frame");
        assert_eq!(rendered[0][6], UNKNOWN, "Δcost on the first frame");
    }

    #[test]
    fn missing_cost_renders_a_dash_never_a_zero() {
        let state = state_with(&[vec![group("claude", 10, None)]]);
        let rendered = render_rows(&state);
        assert_eq!(rendered[0][5], UNKNOWN);
        assert!(!rendered[0][5].contains('0'));
    }

    #[test]
    fn an_over_budget_source_row_is_flagged() {
        let mut state = state_with(&[vec![group("opencode", 10, Some(6.0))]]);
        state.app.budgets = vec![evaluated("opencode", BudgetState::Over)];
        let rendered = render_rows(&state);
        assert!(rendered[0][0].starts_with("⚠ "), "{:?}", rendered[0]);
    }

    #[test]
    fn signed_deltas_carry_their_sign() {
        let state = state_with(&[
            vec![group("opencode", 1000, Some(1.0))],
            vec![group("opencode", 2500, Some(1.25))],
        ]);
        let rendered = render_rows(&state);
        assert_eq!(rendered[0][4], "+1.5K");
        assert_eq!(rendered[0][6], "+0.250000");
    }

    #[test]
    fn a_zero_delta_renders_zero_not_a_dash() {
        let state = state_with(&[
            vec![group("opencode", 1000, Some(1.0))],
            vec![group("opencode", 1000, Some(1.0))],
        ]);
        let rendered = render_rows(&state);
        assert_eq!(rendered[0][4], "0");
        assert_eq!(rendered[0][6], "+0.000000");
    }

    #[test]
    fn table_header_names_the_active_grouping_dimension() {
        let cells = table_header(GroupBy::Project);
        assert_eq!(cells[0], "project");
        assert_eq!(cells[1], "sessions");
        assert_eq!(cells[6], "Δcost");

        assert_eq!(table_header(GroupBy::Model)[0], "model");
        assert_eq!(table_header(GroupBy::Source)[0], "source");
    }

    /// Flatten every rendered row into cell strings, so row content is
    /// assertable without a terminal backend.
    fn render_rows(state: &WatchTuiState) -> Vec<Vec<String>> {
        state
            .app
            .result
            .as_ref()
            .map(|r| r.groups.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|g| {
                row_cells(&state.app, g)
                    .into_iter()
                    .map(|(text, _)| text)
                    .collect()
            })
            .collect()
    }
}

use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use super::app::{App, TuiState, View};
use crate::source::SourceStatus;

// ---------------------------------------------------------------------------
// Palette — a calm dark theme so the data, not the chrome, draws the eye.
// ---------------------------------------------------------------------------
pub(crate) const BG: Color = Color::Rgb(15, 17, 23); // app background
pub(crate) const SURFACE: Color = Color::Rgb(20, 23, 33); // raised panels / zebra stripe
pub(crate) const HILITE: Color = Color::Rgb(38, 52, 74); // selected row
pub(crate) const BORDER: Color = Color::Rgb(42, 47, 58); // panel borders
pub(crate) const TITLE: Color = Color::Rgb(139, 149, 168); // block titles
pub(crate) const TEXT: Color = Color::Rgb(201, 209, 217); // primary text
pub(crate) const MUTED: Color = Color::Rgb(110, 118, 129); // secondary text
pub(crate) const ACCENT: Color = Color::Rgb(86, 212, 221); // cyan — primary accent
pub(crate) const ACCENT2: Color = Color::Rgb(199, 146, 234); // purple — grouping accent
pub(crate) const GREEN: Color = Color::Rgb(126, 231, 135);
pub(crate) const BLUE: Color = Color::Rgb(121, 192, 255);
pub(crate) const YELLOW: Color = Color::Rgb(227, 179, 65);
pub(crate) const RED: Color = Color::Rgb(255, 123, 114);

/// Per-source brand color used across the sources panel and the table's
/// `Source` column so a group's origin is identifiable at a glance.
pub(crate) fn source_color(name: &str) -> Color {
    match name {
        "claude" => Color::Rgb(255, 138, 76),
        "opencode" => BLUE,
        "omp" => GREEN,
        "kilo" => YELLOW,
        "mixed" => MUTED,
        _ => MUTED,
    }
}

/// Format a token count with a magnitude-appropriate unit: raw below 1K,
/// then K, M, B (decimal bases). Whole values drop the decimal: 999, 12K,
/// 1.5M, 2B. If rounding pushes the value into the next unit (e.g. 999_999 →
/// "1000K"), it is re-scaled so the unit boundary is correct ("1M").
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
    // Rounding crossed a unit boundary (e.g. 999_999 -> "1000K"); promote it.
    if text == "1000" {
        return match suffix {
            "K" => "1M".to_string(),
            "M" => "1B".to_string(),
            _ => "1000B".to_string(),
        };
    }
    format!("{}{}", text, suffix)
}

/// Claude Code transcripts do not expose cache-read / cache-write tokens, so
/// every Claude group aggregates to a hard zero there. Render that as an
/// explicit "No data" rather than a misleading `0`. Other sources report these
/// fields, so they keep their real (possibly zero) value.
fn cache_cell(source: &str, value: u64) -> String {
    if source == "claude" {
        "No data".to_string()
    } else {
        format_tokens(value)
    }
}

pub fn render(frame: &mut Frame, state: &mut TuiState) {
    let area = frame.area();

    // Base background fills the whole frame so every panel sits on a uniform
    // color regardless of how the buffer is cleared between draws.
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);

    let chunks = Layout::default()
        .constraints([
            Constraint::Length(4), // header
            Constraint::Length(3), // sources
            Constraint::Min(8),    // table
            Constraint::Length(2), // footer
        ])
        .split(area);

    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_sources(&state.app.source_statuses), chunks[1]);
    let table = match state.app.view {
        View::Detail => render_detail_table(state),
        View::Groups => render_table(state),
    };
    frame.render_stateful_widget(table, chunks[2], state.table_state_mut());
    frame.render_widget(render_footer(&state.app), chunks[3]);
}

fn panel(block_title: &'static str) -> Block<'static> {
    panel_with_title(format!(" {} ", block_title))
}

fn panel_with_title(title: String) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG))
        .title(Line::from(Span::styled(title, Style::default().fg(TITLE))))
}

fn render_header(state: &TuiState) -> Paragraph<'_> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  usage", Style::default().fg(TITLE)),
    ])];

    if let Some(ref result) = state.app.result {
        lines.push(Line::from(vec![
            Span::styled("Sessions ", Style::default().fg(MUTED)),
            Span::styled(
                result.grand_sessions.to_string(),
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Messages ", Style::default().fg(MUTED)),
            Span::styled(
                result.grand_messages.to_string(),
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   In ", Style::default().fg(MUTED)),
            Span::styled(
                format_tokens(result.grand_totals.input),
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Out ", Style::default().fg(MUTED)),
            Span::styled(
                format_tokens(result.grand_totals.output),
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("grouped by ", Style::default().fg(MUTED)),
            Span::styled(
                state.app.group_by.label().to_string(),
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
            ),
            // Budget indicator rides on the same line so the header height is
            // unchanged; only present when budgets are configured.
            match state.app.budget_indicator() {
                Some(indicator) => {
                    let over = indicator.contains("over");
                    Span::styled(
                        format!("   {}", indicator),
                        Style::default()
                            .fg(if over { RED } else { MUTED })
                            .add_modifier(Modifier::BOLD),
                    )
                }
                None => Span::raw(""),
            },
        ]));
    }

    Paragraph::new(lines).block(panel("llmhelper"))
}

fn render_sources(statuses: &[SourceStatus]) -> Paragraph<'_> {
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
                format!(" {}", s.record_count),
                Style::default().fg(MUTED),
            ));
        }
        spans.push(Span::raw("    "));
    }

    Paragraph::new(Line::from(spans)).block(panel("sources"))
}

fn render_detail_table(state: &TuiState) -> Table<'static> {
    let detail = match &state.app.detail {
        Some(d) => d,
        None => {
            return Table::new(
                vec![Row::new(vec![Cell::from(" No detail data. ")])
                    .style(Style::default().fg(MUTED).bg(BG))],
                vec![Constraint::Min(20)],
            )
            .header(Row::new(vec![Cell::from("")]))
            .block(panel("detail"));
        }
    };

    let block = panel_with_title(format!(
        " {} ({} sessions) ",
        detail.key,
        detail.records.len()
    ));
    let headers = [
        "Session", "Source", "Project", "Model", "Started", "Ended", "Messages", "Input", "Output",
        "Cache R", "Cache W", "Cost",
    ];
    let col_fg = [
        ACCENT, ACCENT2, TEXT, TEXT, MUTED, MUTED, YELLOW, GREEN, BLUE, ACCENT, YELLOW, RED,
    ];
    let col_right = [
        false, false, false, false, false, false, true, true, true, true, true, true,
    ];
    let header_cells: Vec<Cell> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let style = Style::default()
                .fg(col_fg[i])
                .add_modifier(Modifier::BOLD)
                .bg(SURFACE);
            let mut cell = Cell::new(*h).style(style);
            if col_right[i] {
                cell = Cell::from(Text::from(*h).alignment(Alignment::Right)).style(style);
            }
            cell
        })
        .collect();
    let header_row = Row::new(header_cells).height(1);

    let col_widths = [
        Constraint::Min(18),
        Constraint::Min(8),
        Constraint::Min(18),
        Constraint::Min(12),
        Constraint::Min(14),
        Constraint::Min(14),
        Constraint::Min(8),
        Constraint::Min(6),
        Constraint::Min(6),
        Constraint::Min(8),
        Constraint::Min(8),
        Constraint::Min(6),
    ];

    let selected = state.detail_state.selected();
    let rows: Vec<Row> = detail
        .records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let is_sel = Some(i) == selected;
            let row_bg = if is_sel {
                HILITE
            } else if i % 2 == 1 {
                SURFACE
            } else {
                BG
            };
            let style = |fg: Color| Style::default().fg(fg).bg(row_bg);
            let ended = r
                .ended_at
                .map(|e| e.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "-".to_string());
            let cost = r
                .cost
                .map(|c| format!("{:.4}", c))
                .unwrap_or_else(|| "-".to_string());

            let cells: Vec<Cell> = vec![
                Cell::new(format!(
                    "{} {}",
                    if is_sel { "▶" } else { " " },
                    r.session_id
                ))
                .style(
                    Style::default()
                        .fg(ACCENT)
                        .add_modifier(if is_sel {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        })
                        .bg(row_bg),
                ),
                Cell::new(r.source.clone()).style(style(source_color(&r.source))),
                Cell::new(r.project.chars().take(20).collect::<String>()).style(style(TEXT)),
                Cell::new(r.model.chars().take(14).collect::<String>()).style(style(TEXT)),
                Cell::new(r.started_at.format("%Y-%m-%d %H:%M").to_string()).style(style(MUTED)),
                Cell::new(ended).style(style(MUTED)),
                Cell::from(Text::from(r.message_count.to_string()).alignment(Alignment::Right))
                    .style(style(YELLOW)),
                Cell::from(Text::from(format_tokens(r.tokens.input)).alignment(Alignment::Right))
                    .style(style(GREEN)),
                Cell::from(Text::from(format_tokens(r.tokens.output)).alignment(Alignment::Right))
                    .style(style(BLUE)),
                Cell::from(
                    Text::from(cache_cell(&r.source, r.tokens.cache_read))
                        .alignment(Alignment::Right),
                )
                .style(style(ACCENT)),
                Cell::from(
                    Text::from(cache_cell(&r.source, r.tokens.cache_write))
                        .alignment(Alignment::Right),
                )
                .style(style(YELLOW)),
                Cell::from(Text::from(cost).alignment(Alignment::Right)).style(style(RED)),
            ];

            Row::new(cells).height(1)
        })
        .collect();

    Table::new(rows, col_widths)
        .header(header_row)
        .block(block)
        .style(Style::default().bg(BG))
}

fn render_footer(app: &App) -> Paragraph<'_> {
    let keys: &[(&str, &str)] = if app.view == View::Detail {
        &[
            ("↑↓", "select"),
            ("r", "refresh"),
            ("Esc", "back"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("Tab", "group"),
            ("↑↓", "select"),
            ("Enter", "detail"),
            ("r", "refresh"),
            ("q", "quit"),
        ]
    };
    let mut spans = Vec::new();
    for (k, label) in keys {
        spans.push(Span::styled(
            format!(" {} ", k),
            Style::default()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{}    ", label),
            Style::default().fg(MUTED),
        ));
    }

    Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG)),
    )
}

fn render_table(state: &mut TuiState) -> Table<'static> {
    let total_records: usize = state
        .app
        .source_statuses
        .iter()
        .map(|s| s.record_count)
        .sum();

    let block = panel("groups").title(Line::from(vec![
        Span::styled(" groups ", Style::default().fg(TITLE)),
        Span::styled(
            format!("  {} records", total_records),
            Style::default().fg(MUTED),
        ),
    ]));

    let result = match &state.app.result {
        Some(r) => r,
        None => {
            return Table::new(
                vec![Row::new(vec![Cell::from(" No data loaded yet. ")])
                    .style(Style::default().fg(MUTED).bg(BG))],
                vec![Constraint::Min(20)],
            )
            .header(Row::new(vec![Cell::from("")]))
            .block(block);
        }
    };

    // Each column carries its own color so the header doubles as a legend.
    let headers = [
        "Group", "Source", "Sessions", "Messages", "Input", "Output", "Cache R", "Cache W", "Cost",
    ];
    let col_fg = [
        ACCENT, ACCENT2, YELLOW, YELLOW, GREEN, BLUE, ACCENT, YELLOW, RED,
    ];
    // Right-align the numeric columns for easy scanning.
    let col_right = [false, false, true, true, true, true, true, true, true];

    let header_cells: Vec<Cell> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let style = Style::default()
                .fg(col_fg[i])
                .add_modifier(Modifier::BOLD)
                .bg(SURFACE);
            let mut cell = Cell::new(*h).style(style);
            if col_right[i] {
                cell = Cell::from(Text::from(*h).alignment(Alignment::Right)).style(style);
            }
            cell
        })
        .collect();
    let header_row = Row::new(header_cells).height(1);

    let col_widths = [
        Constraint::Percentage(24),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
    ];

    let selected = state.table_state.selected();
    let rows: Vec<Row> = result
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let is_sel = Some(i) == selected;
            let row_bg = if is_sel {
                HILITE
            } else if i % 2 == 1 {
                SURFACE
            } else {
                BG
            };

            let over_budget = state.app.source_is_over_budget(&g.source);

            let num = |s: String, fg: Color, right: bool| -> Cell {
                if right {
                    Cell::from(Text::from(s).alignment(Alignment::Right))
                        .style(Style::default().fg(fg).bg(row_bg))
                } else {
                    Cell::new(s).style(Style::default().fg(fg).bg(row_bg))
                }
            };

            let cells: Vec<Cell> = vec![
                if is_sel {
                    Cell::new(format!("▶ {}", g.key)).style(
                        Style::default()
                            .fg(ACCENT)
                            .add_modifier(Modifier::BOLD)
                            .bg(row_bg),
                    )
                } else if over_budget {
                    // Flag the over-budget source without adding a column: the
                    // two-char marker occupies the same width as the plain
                    // selection gutter, so the layout never shifts.
                    Cell::new(format!("⚠ {}", g.key)).style(
                        Style::default()
                            .fg(RED)
                            .add_modifier(Modifier::BOLD)
                            .bg(row_bg),
                    )
                } else {
                    // Pad with a leading space so the column width does not
                    // shift when the selection marker appears/disappears.
                    Cell::new(format!("  {}", g.key)).style(Style::default().fg(TEXT).bg(row_bg))
                },
                Cell::new(g.source.clone())
                    .style(Style::default().fg(source_color(&g.source)).bg(row_bg)),
                num(g.sessions.to_string(), TEXT, true),
                num(g.messages.to_string(), TEXT, true),
                num(format_tokens(g.tokens.input), GREEN, true),
                num(format_tokens(g.tokens.output), BLUE, true),
                num(cache_cell(&g.source, g.tokens.cache_read), ACCENT, true),
                num(cache_cell(&g.source, g.tokens.cache_write), YELLOW, true),
                num(
                    match g.cost {
                        Some(c) => format!("{:.4}", c),
                        None => "-".to_string(),
                    },
                    RED,
                    true,
                ),
            ];

            Row::new(cells).height(1)
        })
        .collect();

    Table::new(rows, col_widths)
        .header(header_row)
        .block(block)
        .style(Style::default().bg(BG))
}

#[cfg(test)]
mod tests {
    use super::{cache_cell, format_tokens};

    #[test]
    fn formats_by_magnitude() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1K");
        assert_eq!(format_tokens(12_345), "12.3K");
        assert_eq!(format_tokens(999_999), "1M");
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(999_999_999), "1B");
        assert_eq!(format_tokens(1_000_000_000), "1B");
        assert_eq!(format_tokens(2_500_000_000), "2.5B");
    }

    #[test]
    fn claude_cache_columns_show_no_data() {
        assert_eq!(cache_cell("claude", 0), "No data");
        assert_eq!(cache_cell("claude", 5), "No data");
    }

    #[test]
    fn non_claude_cache_columns_render_value() {
        assert_eq!(cache_cell("opencode", 0), "0");
        assert_eq!(cache_cell("omp", 1_200), "1.2K");
        assert_eq!(cache_cell("mixed", 0), "0");
    }
}

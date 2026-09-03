use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::source::SourceStatus;
use super::diff_app::DiffTuiState;

// ---------------------------------------------------------------------------
// Palette — shares the same calm dark theme as the usage TUI so both commands
// feel like one app.
// ---------------------------------------------------------------------------
const BG: Color = Color::Rgb(15, 17, 23);
const SURFACE: Color = Color::Rgb(20, 23, 33);
const HILITE: Color = Color::Rgb(38, 52, 74);
const BORDER: Color = Color::Rgb(42, 47, 58);
const TITLE: Color = Color::Rgb(139, 149, 168);
const TEXT: Color = Color::Rgb(201, 209, 217);
const MUTED: Color = Color::Rgb(110, 118, 129);
const ACCENT: Color = Color::Rgb(86, 212, 221);
const ACCENT2: Color = Color::Rgb(199, 146, 234);
const GREEN: Color = Color::Rgb(126, 231, 135);
const BLUE: Color = Color::Rgb(121, 192, 255);
const YELLOW: Color = Color::Rgb(227, 179, 65);
const RED: Color = Color::Rgb(255, 123, 114);

fn source_color(name: &str) -> Color {
    match name {
        "claude" => Color::Rgb(255, 138, 76),
        "opencode" => BLUE,
        "omp" => GREEN,
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

fn fmt_ts(t: &DateTime<Utc>) -> String {
    t.format("%m-%d %H:%M:%S UTC").to_string()
}

pub fn render(frame: &mut Frame, state: &mut DiffTuiState) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);

    let chunks = Layout::default()
        .constraints([
            Constraint::Length(6),  // header: title + window bounds
            Constraint::Length(3),  // sources
            Constraint::Min(8),     // table
            Constraint::Length(2),  // footer
        ])
        .split(area);

    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_sources(&state.app.source_statuses), chunks[1]);
    frame.render_widget(render_table(state), chunks[2]);
    frame.render_widget(render_footer(), chunks[3]);
}

fn render_header(state: &DiffTuiState) -> Paragraph<'_> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  diff", Style::default().fg(TITLE)),
    ])];

    if let (Some(prev_start), Some(prev_end), Some(curr_start), Some(curr_end)) = (
        &state.app.window_prev_start,
        &state.app.window_prev_end,
        &state.app.window_curr_start,
        &state.app.window_curr_end,
    ) {
        lines.push(Line::from(vec![
            Span::styled("prev  ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} → {}", fmt_ts(prev_start), fmt_ts(prev_end)),
                Style::default().fg(MUTED),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("curr  ", Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} → {}", fmt_ts(curr_start), fmt_ts(curr_end)),
                Style::default().fg(MUTED),
            ),
        ]));
    }

    let prev = state.app.prev_total_sessions();
    let curr = state.app.curr_total_sessions();
    let delta = curr as i64 - prev as i64;

    let mut total_line = vec![
        Span::styled("grouped by ", Style::default().fg(MUTED)),
        Span::styled(
            state.app.group_by.label().to_string(),
            Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
        ),
        Span::styled("    ", Style::default().fg(MUTED)),
        Span::styled("sessions ", Style::default().fg(MUTED)),
    ];
    total_line.extend(vec![
        Span::styled(
            prev.to_string(),
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" → ", Style::default().fg(MUTED)),
        Span::styled(
            curr.to_string(),
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  Δ ", Style::default().fg(MUTED)),
        Span::styled(
            i64_disp(delta),
            Style::default()
                .fg(if delta >= 0 { GREEN } else { RED })
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    lines.push(Line::from(total_line));

    Paragraph::new(lines).block(panel("llmhelper diff"))
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
            spans.push(Span::styled(
                format!(" {}", err),
                Style::default().fg(RED),
            ));
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

fn render_footer() -> Paragraph<'static> {
    let keys: &[(&str, &str)] = &[
        ("Tab", "group"),
        ("↑↓", "select"),
        ("r", "refresh"),
        ("q", "quit"),
    ];
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

fn fmt_tokens(v: u64) -> String {
    if v < 1_000 {
        return v.to_string();
    }
    let (value, suffix) = if v < 1_000_000 {
        (v as f64 / 1_000.0, "K")
    } else if v < 1_000_000_000 {
        (v as f64 / 1_000_000.0, "M")
    } else {
        (v as f64 / 1_000_000_000.0, "B")
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

fn fmt_delta_signed(v: i64) -> String {
    if v >= 0 {
        format!("+{}", fmt_tokens(v as u64))
    } else {
        format!("−{}", fmt_tokens((-v) as u64))
    }
}

fn i64_disp(v: i64) -> String {
    fmt_delta_signed(v)
}

fn fmt_cell(v: Option<u64>) -> String {
    v.map(fmt_tokens).unwrap_or_else(|| "—".to_string())
}

fn fmt_pct(pct: Option<f64>) -> String {
    pct.map(|v| format!("{:+.1}%", v)).unwrap_or_default()
}

fn render_table(state: &mut DiffTuiState) -> Table<'static> {
    let total_records: usize = state
        .app
        .source_statuses
        .iter()
        .map(|s| s.record_count)
        .sum();

    let block = panel("groups").title(Line::from(vec![
        Span::styled(" diff ", Style::default().fg(TITLE)),
        Span::styled(
            format!("  {} rows  ·  {} records", state.app.rows.len(), total_records),
            Style::default().fg(MUTED),
        ),
    ]));

    if state.app.rows.is_empty() {
        return Table::new(
            vec![Row::new(vec![Cell::from(" No groups to compare. ")]).style(
                Style::default().fg(MUTED).bg(BG),
            )],
            vec![Constraint::Min(20)],
        )
        .header(Row::new(vec![Cell::from("")]))
        .block(block);
    }

    use crate::diff::Presence;

    let headers = [
        "Key",
        "Present",
        "P·In",
        "C·In",
        "ΔIn",
        "ΔIn%",
        "P·Out",
        "C·Out",
        "ΔOut",
        "ΔOut%",
        "ΔSess",
    ];
    let col_fg = [
        ACCENT, MUTED,
        YELLOW, YELLOW,
        GREEN, GREEN,
        BLUE, BLUE,
        RED, RED,
        ACCENT,
    ];
    let col_right = [false, false,
        true, true, true, true,
        true, true, true, true, true];

    let header_cells: Vec<Cell> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let style = Style::default()
                .fg(col_fg[i])
                .add_modifier(Modifier::BOLD)
                .bg(SURFACE);
            if col_right[i] {
                Cell::from(Text::from(*h).alignment(Alignment::Right)).style(style)
            } else {
                Cell::new(*h).style(style)
            }
        })
        .collect();
    let header_row = Row::new(header_cells).height(1);

    let col_widths = [
        Constraint::Percentage(22), // Key
        Constraint::Length(10),      // Present
        Constraint::Length(8), Constraint::Length(8),
        Constraint::Length(7), Constraint::Length(7),
        Constraint::Length(8), Constraint::Length(8),
        Constraint::Length(7), Constraint::Length(7),
        Constraint::Length(7),
    ];

    let selected = state.table_state.selected();
    let rows: Vec<Row> = state
        .app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_sel = Some(i) == selected;
            let row_bg = if is_sel {
                HILITE
            } else if i % 2 == 1 {
                SURFACE
            } else {
                BG
            };

            let num = |s: String, fg: Color, right: bool| -> Cell {
                if right {
                    Cell::from(Text::from(s).alignment(Alignment::Right))
                        .style(Style::default().fg(fg).bg(row_bg))
                } else {
                    Cell::new(s).style(Style::default().fg(fg).bg(row_bg))
                }
            };

            let key_display = match row.presence {
                Presence::New => format!("▲ {}", row.key),
                Presence::Removed => format!("▼ {}", row.key),
                Presence::Both => row.key.clone(),
            };
            let key_color = match row.presence {
                Presence::New => GREEN,
                Presence::Removed => RED,
                Presence::Both => TEXT,
            };

            let prev_in = fmt_cell(row.prev.as_ref().map(|s| s.tokens.input));
            let curr_in = fmt_cell(row.curr.as_ref().map(|s| s.tokens.input));
            let delta_in = fmt_delta_signed(row.delta.tokens.input);
            let delta_in_pct = fmt_pct(row.delta.pct.as_ref().and_then(|p| p.input));

            let prev_out = fmt_cell(row.prev.as_ref().map(|s| s.tokens.output));
            let curr_out = fmt_cell(row.curr.as_ref().map(|s| s.tokens.output));
            let delta_out = fmt_delta_signed(row.delta.tokens.output);
            let delta_out_pct = fmt_pct(row.delta.pct.as_ref().and_then(|p| p.output));

            let delta_sess = fmt_delta_signed(row.delta.sessions);

            let presence_str = row.presence.to_string();
            let presence_color = match row.presence {
                Presence::New => GREEN,
                Presence::Removed => RED,
                Presence::Both => MUTED,
            };

            let cells: Vec<Cell> = vec![
                if is_sel {
                    Cell::new(format!("▶ {}", key_display)).style(
                        Style::default()
                            .fg(key_color)
                            .add_modifier(Modifier::BOLD)
                            .bg(row_bg),
                    )
                } else {
                    Cell::new(key_display).style(
                        Style::default().fg(key_color).bg(row_bg),
                    )
                },
                Cell::new(presence_str).style(
                    Style::default().fg(presence_color).bg(row_bg),
                ),
                num(prev_in, YELLOW, true),
                num(curr_in, YELLOW, true),
                num(delta_in, GREEN, true),
                num(delta_in_pct, GREEN, true),
                num(prev_out, BLUE, true),
                num(curr_out, BLUE, true),
                num(delta_out, RED, true),
                num(delta_out_pct, RED, true),
                num(delta_sess, ACCENT, true),
            ];

            Row::new(cells).height(1)
        })
        .collect();

    Table::new(rows, col_widths)
        .header(header_row)
        .block(block)
        .style(Style::default().bg(BG))
}

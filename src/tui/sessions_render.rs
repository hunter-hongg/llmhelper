use crate::domain::record::Record;
use crate::source::SourceStatus;
use crate::tui::render::{
    format_tokens, source_color, ACCENT, ACCENT2, BG, BLUE, BORDER, GREEN, HILITE, MUTED, RED,
    SURFACE, TEXT, TITLE, YELLOW,
};
use crate::tui::sessions_app::{SessionsTuiState, SessionsView};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

pub fn render(frame: &mut Frame, state: &mut SessionsTuiState) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);
    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_sources(&state.app.source_statuses), chunks[1]);
    match state.app.view {
        SessionsView::Detail => {
            if let Some(detail) = state.app.detail.as_ref() {
                frame.render_widget(render_detail(detail), chunks[2]);
            }
        }
        SessionsView::List => {
            let table = render_table(state);
            frame.render_stateful_widget(table, chunks[2], &mut state.table_state);
        }
    }
    frame.render_widget(render_footer(state), chunks[3]);
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

fn render_header(state: &SessionsTuiState) -> Paragraph<'_> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  sessions", Style::default().fg(TITLE)),
    ])];
    match state.app.view {
        SessionsView::List => {
            let count = state.app.records.len();
            lines.push(Line::from(vec![
                Span::styled("Sessions ", Style::default().fg(MUTED)),
                Span::styled(
                    count.to_string(),
                    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        SessionsView::Detail => {
            let id = state
                .app
                .detail
                .as_ref()
                .map(|r| r.session_id.chars().take(56).collect::<String>())
                .unwrap_or_else(|| "-".to_string());
            lines.push(Line::from(vec![
                Span::styled("Session ", Style::default().fg(MUTED)),
                Span::styled(id, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            ]));
        }
    }
    Paragraph::new(lines).block(panel("llmhelper"))
}

fn render_sources(statuses: &[SourceStatus]) -> Paragraph<'_> {
    let mut spans = Vec::new();
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
        spans.push(Span::styled(
            format!(" {}", s.record_count),
            Style::default().fg(MUTED),
        ));
        spans.push(Span::raw("    "));
    }
    Paragraph::new(Line::from(spans)).block(panel("sources"))
}

fn render_footer(state: &SessionsTuiState) -> Paragraph<'_> {
    let keys: &[(&str, &str)] = match state.app.view {
        SessionsView::Detail => &[("r", "refresh"), ("Esc", "back"), ("q", "quit")],
        SessionsView::List => &[
            ("↑↓", "select"),
            ("r", "refresh"),
            ("Enter", "detail"),
            ("q", "quit"),
        ],
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

fn render_detail(record: &Record) -> Table<'static> {
    let title = format!(
        " {} ",
        record.session_id.chars().take(48).collect::<String>()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG))
        .title(Line::from(Span::styled(title, Style::default().fg(TITLE))));
    let rows = [
        detail_row("Session id", record.session_id.clone(), ACCENT),
        detail_row(
            "Source",
            record.source.clone(),
            source_color(&record.source),
        ),
        detail_row("Project", record.project.clone(), TEXT),
        detail_row("Model", record.model.clone(), TEXT),
        detail_row(
            "Agent",
            record.agent.clone().unwrap_or_else(|| "-".to_string()),
            YELLOW,
        ),
        detail_row("Started", format_timestamp(record.started_at), TEXT),
        detail_row(
            "Ended",
            record
                .ended_at
                .map(format_timestamp)
                .unwrap_or_else(|| "Running".to_string()),
            TEXT,
        ),
        detail_row("Messages", record.message_count.to_string(), YELLOW),
        detail_row("Input", format_tokens(record.tokens.input), GREEN),
        detail_row("Output", format_tokens(record.tokens.output), BLUE),
        detail_row(
            "Cache Read",
            format_tokens(record.tokens.cache_read),
            ACCENT,
        ),
        detail_row(
            "Cache Write",
            format_tokens(record.tokens.cache_write),
            YELLOW,
        ),
        detail_row(
            "Cost",
            record
                .cost
                .map(|c| format!("{:.4}", c))
                .unwrap_or_else(|| "No data".to_string()),
            RED,
        ),
    ];
    Table::new(
        rows.to_vec(),
        vec![Constraint::Percentage(25), Constraint::Percentage(75)],
    )
    .block(block)
    .style(Style::default().bg(BG))
}

fn detail_row(label: &str, value: String, value_color: Color) -> Row<'static> {
    Row::new(vec![
        Cell::new(label.to_string()).style(
            Style::default()
                .fg(MUTED)
                .add_modifier(Modifier::BOLD)
                .bg(SURFACE),
        ),
        Cell::new(value).style(Style::default().fg(value_color)),
    ])
}

fn format_timestamp(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn render_table(state: &mut SessionsTuiState) -> Table<'static> {
    let block = panel("sessions");
    let records = &state.app.records;
    if records.is_empty() {
        return Table::new(
            vec![
                Row::new(vec![Cell::from(" No sessions loaded ")].into_iter())
                    .style(Style::default().fg(MUTED).bg(BG)),
            ],
            vec![Constraint::Min(20)],
        )
        .header(Row::new(vec![Cell::from("")]))
        .block(block);
    }
    let headers = [
        "Source", "Project", "Model", "Started", "Ended", "Msgs", "In", "Out", "Cache R",
        "Cache W", "Cost",
    ];
    let col_fg = [
        ACCENT2, TEXT, TEXT, MUTED, MUTED, YELLOW, GREEN, BLUE, ACCENT, YELLOW, RED,
    ];
    let col_right = [
        false, false, false, false, false, true, true, true, true, true, true,
    ];
    let header_cells: Vec<Cell> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let style = Style::default()
                .fg(col_fg[i])
                .add_modifier(Modifier::BOLD)
                .bg(SURFACE);
            Cell::from(Text::from(*h).alignment(if col_right[i] {
                Alignment::Right
            } else {
                Alignment::Left
            }))
            .style(style)
        })
        .collect();
    let header_row = Row::new(header_cells).height(1);
    let col_widths = [
        Constraint::Length(10),
        Constraint::Length(22),
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
    ];
    let selected = state.table_state.selected();
    let rows: Vec<Row> = records
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
            let style = |fg| Style::default().fg(fg).bg(row_bg);
            let started = r.started_at.format("%Y-%m-%d %H:%M").to_string();
            let ended = r
                .ended_at
                .map(|e| e.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            let cost = r
                .cost
                .map(|c| format!("{:.4}", c))
                .unwrap_or("-".to_string());
            Row::new(vec![
                Cell::new(r.source.clone()).style(style(source_color(&r.source))),
                Cell::new(r.project.chars().take(22).collect::<String>()).style(style(TEXT)),
                Cell::new(r.model.chars().take(18).collect::<String>()).style(style(TEXT)),
                Cell::new(started)
                    .style(style(TEXT))
                    .style(Style::default().bg(row_bg).fg(TEXT)),
                Cell::new(ended).style(style(TEXT)),
                Cell::from(Text::from(r.message_count.to_string()).alignment(Alignment::Right))
                    .style(style(YELLOW)),
                Cell::from(Text::from(format_tokens(r.tokens.input)).alignment(Alignment::Right))
                    .style(style(GREEN)),
                Cell::from(Text::from(format_tokens(r.tokens.output)).alignment(Alignment::Right))
                    .style(style(BLUE)),
                Cell::from(
                    Text::from(format_tokens(r.tokens.cache_read)).alignment(Alignment::Right),
                )
                .style(style(ACCENT)),
                Cell::from(
                    Text::from(format_tokens(r.tokens.cache_write)).alignment(Alignment::Right),
                )
                .style(style(YELLOW)),
                Cell::from(Text::from(cost).alignment(Alignment::Right)).style(style(RED)),
            ])
        })
        .collect();
    Table::new(rows, col_widths)
        .header(header_row)
        .block(block)
        .style(Style::default().bg(BG))
}

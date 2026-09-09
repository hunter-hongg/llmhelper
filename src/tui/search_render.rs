use crate::search::SearchHit;
use crate::source::MessageStatus;
use crate::tui::render::{
    source_color, ACCENT, ACCENT2, BG, BORDER, HILITE, MUTED, RED, SURFACE, TEXT, TITLE, YELLOW,
};
use crate::tui::scroll::Scrollable;
use crate::tui::search_app::{SearchTuiState, SearchView};
use crate::tui::viewer::panel;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame,
};

pub fn render(frame: &mut Frame, state: &mut SearchTuiState) {
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
    frame.render_widget(render_sources(&state.app.message_statuses), chunks[1]);
    match state.app.view {
        SearchView::Detail => {
            // Resize before taking the detail borrow: the wrap width and the
            // viewport both mutate the state.
            if state.app.detail.is_some() {
                state.set_detail_width((chunks[2].width as usize).saturating_sub(2));
                state.set_viewport_height((chunks[2].height as usize).saturating_sub(2));
                if let Some(detail) = state.app.detail.as_ref() {
                    frame.render_widget(render_detail(detail, state), chunks[2]);
                }
            }
        }
        SearchView::List => {
            let table = render_table(state);
            frame.render_stateful_widget(table, chunks[2], &mut state.table_state);
        }
    }
    frame.render_widget(render_footer(state), chunks[3]);
}

fn render_header(state: &SearchTuiState) -> Paragraph<'_> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  search", Style::default().fg(TITLE)),
    ])];
    let mut query_spans = vec![
        Span::styled("Query ", Style::default().fg(MUTED)),
        Span::styled(
            state.app.query.clone(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ];
    if state.app.case_sensitive {
        query_spans.push(Span::styled(
            "  case-sensitive",
            Style::default().fg(YELLOW),
        ));
    }
    if let Some(role) = &state.app.role {
        query_spans.push(Span::styled(
            format!("  role:{role}"),
            Style::default().fg(ACCENT2),
        ));
    }
    if !state.app.filters.is_empty() {
        query_spans.push(Span::styled(
            format!("  {}", state.app.filters),
            Style::default().fg(MUTED),
        ));
    }
    lines.push(Line::from(query_spans));
    match state.app.view {
        SearchView::List => {
            lines.push(Line::from(vec![
                Span::styled("Hits ", Style::default().fg(MUTED)),
                Span::styled(
                    state.app.hits.len().to_string(),
                    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        SearchView::Detail => {
            let who = state.app.detail.as_ref().map(|h| {
                format!(
                    "{} / {}",
                    h.source,
                    h.timestamp
                        .map(|t| t.format("%Y-%m-%d %H:%M:%SZ").to_string())
                        .unwrap_or_else(|| "-".to_string())
                )
            });
            lines.push(Line::from(vec![
                Span::styled("Message ", Style::default().fg(MUTED)),
                Span::styled(
                    who.unwrap_or_else(|| "-".to_string()),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
    Paragraph::new(lines).block(panel("llmhelper"))
}

fn render_sources(statuses: &[MessageStatus]) -> Paragraph<'_> {
    let mut spans = Vec::new();
    for s in statuses {
        let ok = s.error.is_none();
        let color = if ok { source_color(&s.name) } else { RED };
        let mark = if ok { "●" } else { "✗" };
        spans.push(Span::styled(
            format!(" {mark} "),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            s.name.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", s.message_count),
            Style::default().fg(MUTED),
        ));
        spans.push(Span::raw("    "));
    }
    Paragraph::new(Line::from(spans)).block(panel("messages"))
}

fn render_footer(state: &SearchTuiState) -> Paragraph<'_> {
    let keys: &[(&str, &str)] = match state.app.view {
        SearchView::Detail => &[
            ("↑↓", "scroll"),
            ("PgUp/PgDn", "page"),
            ("Esc", "back"),
            ("r", "refresh"),
            ("q", "quit"),
        ],
        SearchView::List => &[
            ("↑↓", "select"),
            ("g/G", "top/bottom"),
            ("Enter", "detail"),
            ("r", "refresh"),
            ("q", "quit"),
        ],
    };
    let mut spans = Vec::new();
    for (k, label) in keys {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{label}    "),
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

fn render_detail(hit: &SearchHit, state: &SearchTuiState) -> Paragraph<'static> {
    let time = hit
        .timestamp
        .map(|t| t.format("%Y-%m-%d %H:%M:%SZ").to_string())
        .unwrap_or_else(|| "-".to_string());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG))
        .title(Line::from(vec![
            Span::styled(
                format!(" {} ", hit.source),
                Style::default()
                    .fg(source_color(&hit.source))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} · ", hit.role),
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} · ", hit.session_id.chars().take(28).collect::<String>()),
                Style::default().fg(TITLE),
            ),
            Span::styled(time, Style::default().fg(MUTED)),
            Span::styled(
                format!(" · {} matches", hit.matches),
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            ),
        ]));
    let text = Text::from(
        state
            .detail_lines
            .iter()
            .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(TEXT))))
            .collect::<Vec<_>>(),
    );
    Paragraph::new(text)
        .block(block)
        .style(Style::default().bg(BG))
        .wrap(Wrap { trim: false })
        .scroll((state.scroll as u16, 0))
}

fn render_table(state: &mut SearchTuiState) -> Table<'static> {
    let block = panel("hits");
    let hits = &state.app.hits;
    if hits.is_empty() {
        return Table::new(
            vec![Row::new(vec![Cell::from(" No matches ")].into_iter())
                .style(Style::default().fg(MUTED).bg(BG))],
            vec![Constraint::Min(20)],
        )
        .header(Row::new(vec![Cell::from("")]))
        .block(block);
    }
    let headers = ["#", "Role", "Source", "Project", "Time", "Match", "Snippet"];
    let col_fg = [MUTED, ACCENT2, ACCENT2, TEXT, MUTED, YELLOW, TEXT];
    let col_right = [true, false, false, false, false, true, false];
    let col_widths = [
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(26),
        Constraint::Length(19),
        Constraint::Length(6),
        Constraint::Min(20),
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
    let selected = state.table_state.selected();
    let rows: Vec<Row> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let is_sel = Some(i) == selected;
            let row_bg = if is_sel {
                HILITE
            } else if i % 2 == 1 {
                SURFACE
            } else {
                BG
            };
            let style = |fg: Color| Style::default().fg(fg).bg(row_bg);
            let time = h
                .timestamp
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".to_string());
            Row::new(vec![
                Cell::from(Text::from((i + 1).to_string()).alignment(Alignment::Right))
                    .style(style(MUTED)),
                Cell::new(h.role.chars().take(12).collect::<String>()).style(style(ACCENT2)),
                Cell::new(h.source.clone()).style(style(source_color(&h.source))),
                Cell::new(h.project.chars().take(26).collect::<String>()).style(style(TEXT)),
                Cell::new(time).style(style(TEXT)),
                Cell::from(Text::from(h.matches.to_string()).alignment(Alignment::Right)).style(
                    Style::default()
                        .fg(YELLOW)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Cell::new(h.snippet.clone()).style(style(TEXT)),
            ])
        })
        .collect();
    Table::new(rows, col_widths)
        .header(header_row)
        .block(block)
        .style(Style::default().bg(BG))
}

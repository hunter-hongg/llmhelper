use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};

use crate::aggregator::AggregateResult;
use crate::source::SourceStatus;
use super::app::TuiState;

pub fn render(frame: &mut Frame, state: &mut TuiState) {
    let area = frame.area();

    let chunks = Layout::default()
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);

    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_status_bar(&state.app.source_statuses), chunks[1]);
    frame.render_widget(render_table(state), chunks[2]);

    let footer = Paragraph::new(Line::from(Span::styled(
        " Tab:group ↑↓:select r:refresh q/Esc:quit".to_string(),
        Style::default().fg(Color::Gray),
    )))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(footer, chunks[3]);
}

fn render_header(state: &TuiState) -> Paragraph {
    let mut lines = vec![Line::from(Span::styled(
        " llmhelper usage ".to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];
    if let Some(ref result) = state.app.result {
        lines.push(Line::from(vec![
            Span::raw(" Sessions: "),
            Span::styled(result.grand_sessions.to_string(), Style::default().fg(Color::Yellow)),
            Span::raw(" | Messages: "),
            Span::styled(result.grand_messages.to_string(), Style::default().fg(Color::Yellow)),
            Span::raw(" | Input: "),
            Span::styled(format!("{:.1}K", result.grand_totals.input as f64 / 1000.0), Style::default().fg(Color::Green)),
            Span::raw(" Out: "),
            Span::styled(format!("{:.1}K", result.grand_totals.output as f64 / 1000.0), Style::default().fg(Color::Blue)),
            Span::raw(" | Group: "),
            Span::styled(state.app.group_by.label().to_string(), Style::default().fg(Color::Magenta)),
        ]));
    }
    Paragraph::new(lines).block(Block::default().borders(Borders::BOTTOM))
}

fn render_status_bar(statuses: &[SourceStatus]) -> Paragraph {
    let text: Vec<Span> = statuses
        .iter()
        .map(|s| {
            let color = if s.error.is_none() { Color::Green } else { Color::Red };
            Span::styled(
                format!(" {}({}) ", s.name, s.record_count),
                Style::default().fg(color),
            )
        })
        .collect();
    Paragraph::new(Line::from(text))
}

fn render_table(state: &mut TuiState) -> Table<'static> {
    let result = match &state.app.result {
        Some(r) => r,
        None => {
            return Table::new(
                vec![Row::new(vec!["No data loaded yet."])],
                vec![Constraint::Length(20)],
            )
            .header(Row::new(vec!["Message"]).style(Style::default().fg(Color::Cyan)))
            .block(Block::default().borders(Borders::ALL).title(" Usage "))
        }
    };

    let headers = vec![
        "Group", "Source", "Sessions", "Messages",
        "Input", "Output", "Reasoning", "Cache R", "Cache W", "Cost",
    ];
    let header_row = Row::new(headers)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let col_widths = vec![
        Constraint::Percentage(25),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let rows: Vec<Row> = result
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let style = if Some(i) == state.table_state.selected() {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            let cost_str = match g.cost {
                Some(c) => format!("{:.4}", c),
                None => "-".to_string(),
            };
            Row::new(vec![
                g.key.clone(),
                g.source.clone(),
                g.sessions.to_string(),
                g.messages.to_string(),
                format!("{:.1}K", g.tokens.input as f64 / 1000.0),
                format!("{:.1}K", g.tokens.output as f64 / 1000.0),
                format!("{:.1}K", g.tokens.reasoning as f64 / 1000.0),
                format!("{:.1}K", g.tokens.cache_read as f64 / 1000.0),
                format!("{:.1}K", g.tokens.cache_write as f64 / 1000.0),
                cost_str,
            ])
            .style(style)
        })
        .collect();

    Table::new(rows, col_widths)
        .header(header_row)
        .block(Block::default().borders(Borders::ALL).title(" Groups "))
}

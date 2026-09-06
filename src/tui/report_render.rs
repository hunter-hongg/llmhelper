use crate::tui::report_app::ReportTuiState;
use crate::tui::render::{ACCENT, ACCENT2, BG, BORDER, MUTED, SURFACE, TEXT, TITLE};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, state: &mut ReportTuiState) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(3), // header with scroll indicator
            Constraint::Min(8),    // report body
            Constraint::Length(2), // footer
        ])
        .split(area);

    state.set_viewport_height(chunks[1].height as usize);
    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_body(state), chunks[1]);
    frame.render_widget(render_footer(), chunks[2]);
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

fn render_header(state: &ReportTuiState) -> Paragraph<'_> {
    let indicator = scroll_indicator(state);
    Paragraph::new(Line::from(vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  report", Style::default().fg(TITLE)),
        Span::raw("  "),
        Span::styled(indicator, Style::default().fg(MUTED)),
    ]))
    .block(panel("llmhelper"))
}

fn scroll_indicator(state: &ReportTuiState) -> String {
    if state.lines.is_empty() {
        return "empty".to_string();
    }
    let first = state.scroll + 1;
    let last = (state.scroll + state.viewport_height.max(1)).min(state.lines.len());
    format!("lines {}-{} of {}", first, last, state.lines.len())
}

/// Map one Markdown line to styled spans: headings get accent colors and
/// bold, table separators and rules dim, the truncation note and other meta
/// lines mute, everything else stays body text. Pure function of the input.
fn styled_line(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        let fg = if level == 1 { ACCENT } else { ACCENT2 };
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
        ));
    }
    if trimmed.starts_with('|') {
        let fg = if trimmed.contains("---") { MUTED } else { TEXT };
        return Line::from(Span::styled(line.to_string(), Style::default().fg(fg)));
    }
    if trimmed.chars().all(|c| c == '-') && trimmed.contains('-') {
        return Line::from(Span::styled(line.to_string(), Style::default().fg(MUTED)));
    }
    if crate::report::is_truncation_note(trimmed) {
        return Line::from(Span::styled(line.to_string(), Style::default().fg(MUTED)));
    }
    Line::from(Span::styled(line.to_string(), Style::default().fg(TEXT)))
}

fn render_body(state: &ReportTuiState) -> Paragraph<'static> {
    let text = Text::from(state.lines.iter().map(|l| styled_line(l)).collect::<Vec<_>>());
    Paragraph::new(text)
        .block(panel("report"))
        .style(Style::default().bg(BG))
        .scroll((state.scroll as u16, 0))
}

fn render_footer() -> Paragraph<'static> {
    let keys: &[(&str, &str)] = &[
        ("↑↓", "scroll"),
        ("PgUp/PgDn", "page"),
        ("g/G", "top/bottom"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn first_fg(line: &str) -> Color {
        styled_line(line)
            .spans
            .first()
            .map(|s| s.style.fg.unwrap_or(TEXT))
            .unwrap_or(TEXT)
    }

    fn is_bold(line: &str) -> bool {
        styled_line(line)
            .spans
            .first()
            .map(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .unwrap_or(false)
    }

    #[test]
    fn level_one_heading_is_accent_bold() {
        assert_eq!(first_fg("# llmhelper report"), ACCENT);
        assert!(is_bold("# llmhelper report"));
    }

    #[test]
    fn level_two_heading_is_accent2_bold() {
        assert_eq!(first_fg("## Totals"), ACCENT2);
        assert!(is_bold("## Totals"));
    }

    #[test]
    fn deeper_heading_is_accent2_bold() {
        assert_eq!(first_fg("### notes"), ACCENT2);
        assert!(is_bold("### notes"));
    }

    #[test]
    fn table_separator_is_muted() {
        assert_eq!(first_fg("|---|---|---|"), MUTED);
    }

    #[test]
    fn horizontal_rule_is_muted() {
        assert_eq!(first_fg("---"), MUTED);
        assert_eq!(first_fg("----------"), MUTED);
    }

    #[test]
    fn table_row_is_text() {
        assert_eq!(first_fg("| claude | 1 | 2 |"), TEXT);
        assert!(!is_bold("| claude | 1 | 2 |"));
    }

    #[test]
    fn truncation_note_is_muted() {
        assert_eq!(first_fg("_(+ 2 more — 300 input tokens)_"), MUTED);
    }

    #[test]
    fn plain_text_is_text() {
        assert_eq!(first_fg("no sessions matched the current filters"), TEXT);
        assert_eq!(first_fg("- filters: (none)"), TEXT);
    }

    #[test]
    fn scroll_indicator_counts_lines() {
        let mut state = ReportTuiState::new("a\nb\nc\nd\ne");
        state.set_viewport_height(2);
        assert_eq!(scroll_indicator(&state), "lines 1-2 of 5");
        state.scroll_bottom();
        assert_eq!(scroll_indicator(&state), "lines 4-5 of 5");
    }

    #[test]
    fn scroll_indicator_handles_empty_document() {
        let state = ReportTuiState::new("");
        assert_eq!(scroll_indicator(&state), "empty");
    }
}

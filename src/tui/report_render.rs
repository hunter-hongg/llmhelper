use crate::tui::render::{ACCENT, ACCENT2, BG, BORDER, MUTED, TEXT, TITLE};
use crate::tui::report_app::ReportTuiState;
use crate::tui::scroll::Scrollable;
use crate::tui::viewer::{panel, render_footer};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

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
    let first = state.scroll() + 1;
    let last = (state.scroll() + state.viewport_height().max(1)).min(state.lines.len());
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
        let fg = if is_table_separator(line) {
            MUTED
        } else {
            TEXT
        };
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

/// A Markdown table delimiter row like `|---|---|`.
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('|') && trimmed.contains("---")
}

/// A header row followed by a delimiter row opens a real Markdown table.
fn opens_table(lines: &[String], index: usize) -> bool {
    lines
        .get(index)
        .is_some_and(|l| l.trim_start().starts_with('|'))
        && lines.get(index + 1).is_some_and(|l| is_table_separator(l))
}

/// Turn the raw document lines into styled lines, drawing table blocks as
/// aligned tables instead of raw pipe rows. Output keeps one line per input
/// line so scroll math stays 1:1. Pure function of the input.
fn body_lines(lines: &[String]) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if opens_table(lines, i) {
            let mut end = i + 1;
            while end < lines.len() && lines[end].trim_start().starts_with('|') {
                end += 1;
            }
            out.extend(render_table_block(&lines[i..end]));
            i = end;
        } else {
            out.push(styled_line(&lines[i]));
            i += 1;
        }
    }
    out
}

/// Split a table row into cells, trimming whitespace and unescaping `\|`.
fn split_row(line: &str) -> Vec<String> {
    const ESC: char = '\u{0}';
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner
        .replace("\\|", &ESC.to_string())
        .split('|')
        .map(|cell| cell.trim().replace(ESC, "|"))
        .collect()
}

/// Render header + delimiter + body rows as an aligned table: cells padded to
/// the widest cell per column, `│` column separators, and a dim `┼` rule
/// under the header. Pure function of the input.
fn render_table_block(block: &[String]) -> Vec<Line<'static>> {
    let mut rows: Vec<Vec<String>> = vec![split_row(&block[0])];
    for line in block.iter().skip(2) {
        rows.push(split_row(line));
    }
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; ncols];
    for row in &rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.width());
        }
    }

    let mut out = Vec::with_capacity(block.len());
    for (row_idx, row) in rows.iter().enumerate() {
        let header = row_idx == 0;
        let mut spans = Vec::new();
        for (col, width) in widths.iter().enumerate() {
            if col > 0 {
                spans.push(Span::styled(" │ ".to_string(), Style::default().fg(BORDER)));
            }
            let cell = row.get(col).map(String::as_str).unwrap_or("");
            let style = if header {
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };
            spans.push(Span::styled(cell.to_string(), style));
            let pad = width - cell.width();
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
        }
        out.push(Line::from(spans));
        if header {
            // First/last segments cover "cell + one adjacent space" (w + 1);
            // middle segments sit between two `┼` and must also cover the
            // space on their right (w + 2), or every `┼` after the first
            // drifts left of its `│` column by one char per junction.
            let n = widths.len();
            let rule = widths
                .iter()
                .enumerate()
                .map(|(col, w)| {
                    if col > 0 && col + 1 < n {
                        "─".repeat(w + 2)
                    } else {
                        "─".repeat(w + 1)
                    }
                })
                .collect::<Vec<_>>()
                .join("┼");
            out.push(Line::from(Span::styled(rule, Style::default().fg(MUTED))));
        }
    }
    out
}

fn render_body(state: &ReportTuiState) -> Paragraph<'static> {
    let text = Text::from(body_lines(&state.lines));
    Paragraph::new(text)
        .block(panel("report"))
        .style(Style::default().bg(BG))
        .scroll((state.scroll() as u16, 0))
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

    fn plain(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
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
    fn table_block_aligns_columns_under_box_rule() {
        let lines = vec![
            "| key | sessions |".to_string(),
            "|---|---|".to_string(),
            "| claude | 12 |".to_string(),
            "| omp | 3 |".to_string(),
        ];
        let out = body_lines(&lines);
        assert_eq!(out.len(), lines.len());
        assert_eq!(plain(&out[0]), "key    │ sessions");
        assert_eq!(plain(&out[1]), "───────┼─────────");
        assert_eq!(plain(&out[2]), "claude │ 12      ");
        assert_eq!(plain(&out[3]), "omp    │ 3       ");
    }

    #[test]
    fn table_header_is_accent2_bold_and_rule_muted() {
        let lines = vec![
            "| key | n |".to_string(),
            "|---|---|".to_string(),
            "| a | 1 |".to_string(),
        ];
        let out = body_lines(&lines);
        let header = &out[0].spans[0].style;
        assert_eq!(header.fg, Some(ACCENT2));
        assert!(header.add_modifier.contains(Modifier::BOLD));
        assert_eq!(out[1].spans[0].style.fg, Some(MUTED));
        assert_eq!(out[2].spans[0].style.fg, Some(TEXT));
    }

    #[test]
    fn table_cells_unescape_pipes() {
        let lines = vec![
            "| key | n |".to_string(),
            "|---|---|".to_string(),
            "| a\\|b | 1 |".to_string(),
        ];
        let out = body_lines(&lines);
        assert_eq!(plain(&out[2]), "a|b │ 1");
    }

    #[test]
    fn ragged_table_rows_pad_missing_cells() {
        let lines = vec![
            "| a | b | c |".to_string(),
            "|---|---|---|".to_string(),
            "| x | 1 |".to_string(),
        ];
        let out = body_lines(&lines);
        assert_eq!(out.len(), 3);
        assert_eq!(plain(&out[2]), "x │ 1 │  ");
    }

    #[test]
    fn orphan_pipe_line_stays_raw() {
        let lines = vec!["| just a line".to_string()];
        let out = body_lines(&lines);
        assert_eq!(plain(&out[0]), "| just a line");
    }

    #[test]
    fn multiple_table_blocks_each_rendered() {
        let lines = vec![
            "## Totals".to_string(),
            String::new(),
            "| a | b |".to_string(),
            "|---|---|".to_string(),
            "| 1 | 2 |".to_string(),
            String::new(),
            "| c | d |".to_string(),
            "|---|---|".to_string(),
            "| 3 | 4 |".to_string(),
        ];
        let out = body_lines(&lines);
        assert_eq!(out.len(), lines.len());
        assert_eq!(plain(&out[2]), "a │ b");
        assert_eq!(plain(&out[3]), "──┼──");
        assert_eq!(plain(&out[6]), "c │ d");
        assert_eq!(plain(&out[7]), "──┼──");
    }

    #[test]
    fn rule_junctions_align_with_every_column_separator() {
        let lines = vec![
            "| key | sessions | messages |".to_string(),
            "|---|---|---|".to_string(),
            "| claude | 12 | 40 |".to_string(),
        ];
        let out = body_lines(&lines);
        let header = plain(&out[0]);
        let rule = plain(&out[1]);
        assert_eq!(rule.chars().count(), header.chars().count());
        for (idx, ch) in header.chars().enumerate() {
            if ch == '│' {
                assert_eq!(
                    rule.chars().nth(idx),
                    Some('┼'),
                    "junction {} must sit under its │ at col {}",
                    idx,
                    idx
                );
            }
        }
        assert_eq!(rule, "───────┼──────────┼─────────");
    }

    #[test]
    fn cjk_cells_align_by_display_width() {
        let lines = vec![
            "| key | n |".to_string(),
            "|---|---|".to_string(),
            "| 项目 | 1 |".to_string(),
            "| ab | 2 |".to_string(),
        ];
        let out = body_lines(&lines);
        // 项目 is display width 4, same column as ab (width 2): both pad to 4.
        assert_eq!(plain(&out[2]), "项目 │ 1");
        assert_eq!(plain(&out[3]), "ab   │ 2");
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

use crate::tui::render::{ACCENT, BG, BORDER, MUTED, SURFACE, TEXT, TITLE};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
};

/// Rounded panel shared by every TUI region: same border style, same title
/// style, so panels never drift apart between subcommands.
pub fn panel(block_title: &'static str) -> Block<'static> {
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

/// Navigation keys for the static scrollable viewers.
const NAV_KEYS: &[(&str, &str)] = &[
    ("↑↓", "scroll"),
    ("PgUp/PgDn", "page"),
    ("g/G", "top/bottom"),
    ("q", "quit"),
];

/// Standard navigation legend for the static scrollable viewers.
pub fn render_footer() -> Paragraph<'static> {
    Paragraph::new(footer_line()).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG)),
    )
}

/// Footer content: one styled key/label pair per navigation shortcut.
fn footer_line() -> Line<'static> {
    let mut spans = Vec::new();
    for (k, label) in NAV_KEYS {
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
    Line::from(spans)
}

/// Split a plain-text body into equally-styled lines for a body panel.
pub fn plain_body(lines: &[String]) -> Text<'static> {
    let lines = lines
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(TEXT))))
        .collect::<Vec<_>>();
    Text::from(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn footer_lists_every_nav_key() {
        let out = plain(&footer_line());
        for (k, _) in NAV_KEYS {
            assert!(out.contains(k), "footer missing key {}", k);
        }
        for (_, label) in NAV_KEYS {
            assert!(out.contains(label), "footer missing label {}", label);
        }
    }

    #[test]
    fn footer_keys_are_bold_accent_labels_muted() {
        let spans = footer_line().spans;
        assert_eq!(spans.len(), NAV_KEYS.len() * 2);
        for (k, _) in NAV_KEYS {
            let key_span = spans
                .iter()
                .find(|s| s.content.as_ref() == format!(" {} ", k))
                .unwrap();
            assert_eq!(key_span.style.fg, Some(ACCENT));
            assert!(key_span.style.add_modifier.contains(Modifier::BOLD));
        }
        for (_, label) in NAV_KEYS {
            let label_span = spans
                .iter()
                .find(|s| s.content.as_ref() == format!("{}    ", label))
                .unwrap();
            assert_eq!(label_span.style.fg, Some(MUTED));
        }
    }

    #[test]
    fn plain_body_wraps_each_line_as_text() {
        let text = plain_body(&["a".to_string(), "b".to_string()]);
        assert_eq!(text.lines.len(), 2);
        assert_eq!(plain(&text.lines[1]), "b");
    }

    #[test]
    fn plain_body_lines_are_all_body_text() {
        let text = plain_body(&["a".to_string()]);
        assert_eq!(text.lines[0].spans[0].style.fg, Some(TEXT));
    }
}

use crate::output::format_tokens;
use crate::tui::render::{ACCENT, BG, MUTED, TITLE};
use crate::tui::request_app::RequestTuiState;
use crate::tui::scroll::Scrollable;
use crate::tui::viewer::{panel, plain_body, render_footer};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render(frame: &mut Frame, state: &mut RequestTuiState) {
    let area = frame.area();
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(BG)),
        area,
    );
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);

    state.set_viewport_height(chunks[1].height as usize);
    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_body(state), chunks[1]);
    frame.render_widget(render_footer(), chunks[2]);
}

fn render_header(state: &RequestTuiState) -> Paragraph<'_> {
    Paragraph::new(header_line(state)).block(panel("request"))
}

/// Header content: identity on the left, timing and tokens on the right. The
/// endpoint is the host only, so the API key can never reach the TUI.
fn header_line(state: &RequestTuiState) -> Line<'static> {
    let meta = &state.meta;
    Line::from(vec![
        Span::styled("llmhelper", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("  request"),
        Span::raw("  "),
        Span::styled(
            format!("model: {}", meta.model),
            Style::default().fg(TITLE),
        ),
        Span::raw("  "),
        Span::styled(
            format!("endpoint: {}", meta.host),
            Style::default().fg(MUTED),
        ),
        Span::raw("  "),
        Span::styled(
            format!("time: {} ms", meta.duration_ms),
            Style::default().fg(MUTED),
        ),
        Span::raw("  "),
        Span::styled(
            format!("tokens: {}", usage_text(meta.usage_tokens)),
            Style::default().fg(MUTED),
        ),
    ])
}

/// Token count for the header, dimmed dash when the provider omitted usage.
fn usage_text(usage_tokens: Option<u64>) -> String {
    usage_tokens.map(format_tokens).unwrap_or_else(|| "—".to_string())
}

fn render_body(state: &RequestTuiState) -> Paragraph<'static> {
    Paragraph::new(plain_body(&state.body_lines))
        .block(panel("response"))
        .style(Style::default().bg(BG))
        .scroll((state.scroll() as u16, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::request_app::RequestMeta;

    fn state(usage: Option<u64>) -> RequestTuiState {
        RequestTuiState::new(
            RequestMeta {
                model: "gpt-4".to_string(),
                host: "api.example.com".to_string(),
                duration_ms: 1234,
                usage_tokens: usage,
            },
            "hello",
        )
    }

    fn plain(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn header_shows_model_host_time_and_tokens() {
        let out = plain(&header_line(&state(Some(30))));
        assert!(out.contains("llmhelper"));
        assert!(out.contains("request"));
        assert!(out.contains("model: gpt-4"));
        assert!(out.contains("endpoint: api.example.com"));
        assert!(out.contains("time: 1234 ms"));
        assert!(out.contains("tokens: 30"));
    }

    #[test]
    fn header_shows_dash_when_usage_absent() {
        assert!(plain(&header_line(&state(None))).contains("tokens: —"));
    }

    #[test]
    fn usage_text_uses_magnitude_formatting() {
        assert_eq!(usage_text(Some(1_500)), "1.5K");
        assert_eq!(usage_text(None), "—");
    }

    #[test]
    fn header_first_span_is_bold_accent() {
        let first = &header_line(&state(None)).spans[0];
        assert_eq!(first.style.fg, Some(ACCENT));
        assert!(first.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn render_body_wraps_placeholder_for_empty_response() {
        let state = RequestTuiState::new(RequestMeta::default(), "   ");
        assert_eq!(state.body_lines, &["(empty response)".to_string()]);
        assert_eq!(plain_body(&state.body_lines).lines.len(), 1);
    }
}

use crate::output::format_tokens;
use crate::tui::render::{ACCENT, BG, MUTED, TITLE};
use crate::tui::request_app::{GenerationState, RequestTuiState, ThinkingView};
use crate::tui::scroll::Scrollable;
use crate::tui::viewer::{panel, plain_body, render_footer_with};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render(frame: &mut Frame, state: &mut RequestTuiState) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);
    let show_reasoning =
        state.capture_on && state.has_reasoning() && state.view != ThinkingView::Answer;
    let show_answer = state.view != ThinkingView::ThinkingOnly;

    // Split the body region into answer and reasoning panes when both are
    // shown; otherwise render a single pane as before.
    let chunks = if state.interactive {
        Layout::default()
            .constraints([
                Constraint::Length(3),
                Constraint::Min(6),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area)
    } else {
        Layout::default()
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(2),
            ])
            .split(area)
    };

    state.set_viewport_height(chunks[1].height as usize);
    state.reasoning_viewport_height = chunks[1].height as usize;
    frame.render_widget(render_header(state), chunks[0]);

    if show_answer && show_reasoning {
        let body = Layout::default()
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(chunks[1]);
        frame.render_widget(render_body(state), body[0]);
        frame.render_widget(render_reasoning(state), body[1]);
    } else if show_answer {
        frame.render_widget(render_body(state), chunks[1]);
    } else {
        frame.render_widget(render_reasoning(state), chunks[1]);
    }

    if state.interactive {
        frame.render_widget(render_footer(state), chunks[2]);
        frame.render_widget(render_input(state), chunks[3]);
    } else {
        frame.render_widget(render_footer(state), chunks[2]);
    }
}

/// Footer for the request viewer: the shared navigation keys, plus `t` for
/// cycling the channel view when reasoning capture is on.
fn render_footer(state: &RequestTuiState) -> ratatui::widgets::Paragraph<'static> {
    let extra: &[(&str, &str)] = if state.capture_on && !state.interactive {
        &[("t", "view")]
    } else {
        &[]
    };
    render_footer_with(extra)
}

/// Interactive input line: a prompt glyph, the typed text, and a trailing
/// block so the caret position is visible in a non-capturing terminal.
fn render_input(state: &RequestTuiState) -> Paragraph<'static> {
    Paragraph::new(input_line(state))
        .block(panel("input"))
        .style(Style::default().bg(BG))
}

fn input_line(state: &RequestTuiState) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "> ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(state.input.clone()),
        Span::styled("█", Style::default().fg(ACCENT)),
    ])
}

fn render_header(state: &RequestTuiState) -> Paragraph<'_> {
    Paragraph::new(header_line(state)).block(panel("request"))
}

/// Header content: identity on the left, timing and tokens on the right. The
/// endpoint is the host only, so the API key can never reach the TUI.
fn header_line(state: &RequestTuiState) -> Line<'static> {
    let meta = &state.meta;
    let mut spans = vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  request"),
        Span::raw("  "),
        Span::styled(format!("model: {}", meta.model), Style::default().fg(TITLE)),
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
    ];
    match meta.stream_state {
        crate::tui::request_app::StreamState::Live => {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("stream: live", Style::default().fg(ACCENT)));
        }
        crate::tui::request_app::StreamState::Done => {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("stream: done", Style::default().fg(MUTED)));
        }
        crate::tui::request_app::StreamState::Off => {}
    }
    if let Some(style) = gen_state_style(meta.gen_state) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("gen: {}", meta.gen_state.label()),
            style,
        ));
    }
    Line::from(spans)
}

/// Style for the generation-state span. In-flight phases are accented so the
/// header reads as "busy"; a settled `Done` is muted, and `Idle` is not shown
/// at all so a run that never streams keeps its existing header.
fn gen_state_style(state: GenerationState) -> Option<Style> {
    match state {
        GenerationState::Idle => None,
        GenerationState::Thinking | GenerationState::Answering => Some(Style::default().fg(ACCENT)),
        GenerationState::Done => Some(Style::default().fg(MUTED)),
    }
}

/// Token count for the header, dimmed dash when the provider omitted usage.
fn usage_text(usage_tokens: Option<u64>) -> String {
    usage_tokens
        .map(format_tokens)
        .unwrap_or_else(|| "—".to_string())
}

fn render_body(state: &RequestTuiState) -> Paragraph<'static> {
    Paragraph::new(plain_body(&state.body_lines))
        .block(panel("response"))
        .style(Style::default().bg(BG))
        .scroll((state.scroll() as u16, 0))
}

fn render_reasoning(state: &RequestTuiState) -> Paragraph<'static> {
    Paragraph::new(plain_body(&state.reasoning_lines))
        .block(panel("thinking"))
        .style(Style::default().bg(BG))
        .scroll((state.reasoning_scroll as u16, 0))
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
                stream_state: crate::tui::request_app::StreamState::Off,
                ..Default::default()
            },
            "hello",
        )
    }

    fn state_with_gen(gen_state: GenerationState) -> RequestTuiState {
        let mut state = state(Some(30));
        state.meta.gen_state = gen_state;
        state
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

    #[test]
    fn header_omits_gen_state_when_idle() {
        assert!(!plain(&header_line(&state_with_gen(GenerationState::Idle))).contains("gen:"));
    }

    #[test]
    fn header_shows_each_active_gen_state_label() {
        for (state, label) in [
            (GenerationState::Thinking, "gen: thinking"),
            (GenerationState::Answering, "gen: answering"),
            (GenerationState::Done, "gen: done"),
        ] {
            assert!(plain(&header_line(&state_with_gen(state))).contains(label));
        }
    }

    #[test]
    fn header_accents_in_flight_gen_state_and_mutes_done() {
        let gen_style = |state| {
            header_line(&state_with_gen(state))
                .spans
                .iter()
                .find(|s| s.content.starts_with("gen:"))
                .and_then(|s| s.style.fg)
        };
        assert_eq!(gen_style(GenerationState::Thinking), Some(ACCENT));
        assert_eq!(gen_style(GenerationState::Answering), Some(ACCENT));
        assert_eq!(gen_style(GenerationState::Done), Some(MUTED));
    }
}

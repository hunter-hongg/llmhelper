use crate::tui::scroll::Scrollable;

/// Header metadata for the request viewer, kept apart from the response body
/// so the render layer never has to touch the raw response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamState {
    #[default]
    Off,
    Live,
    Done,
}

#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    pub model: String,
    /// Endpoint identity only — the host with protocol and path stripped, so
    /// the API key can never appear in the TUI.
    pub host: String,
    pub duration_ms: u128,
    pub usage_tokens: Option<u64>,
    pub stream_state: StreamState,
}

#[derive(Debug, Default)]
pub struct RequestTuiState {
    pub running: bool,
    pub body_lines: Vec<String>,
    pub scroll: usize,
    pub viewport_height: usize,
    pub meta: RequestMeta,
    pub follow: bool,
}

impl RequestTuiState {
    pub fn new(meta: RequestMeta, body: &str) -> Self {
        let body_lines: Vec<String> =
            if body.trim().is_empty() && !matches!(meta.stream_state, StreamState::Live) {
                vec!["(empty response)".to_string()]
            } else if body.trim().is_empty() && matches!(meta.stream_state, StreamState::Live) {
                vec!["".to_string()]
            } else {
                body.lines().map(str::to_string).collect()
            };
        Self {
            running: false,
            body_lines,
            scroll: 0,
            viewport_height: 0,
            meta: meta.clone(),
            follow: matches!(meta.stream_state, StreamState::Live),
        }
    }

    pub fn quit(&mut self) {
        self.running = false;
    }
}

impl Scrollable for RequestTuiState {
    fn scroll(&self) -> usize {
        self.scroll
    }
    fn viewport_height(&self) -> usize {
        self.viewport_height
    }
    fn content_length(&self) -> usize {
        self.body_lines.len()
    }
    fn set_scroll(&mut self, offset: usize) {
        self.scroll = offset;
    }
    fn set_viewport(&mut self, height: usize) {
        self.viewport_height = height;
    }
}

pub struct RequestTuiApp {
    pub terminal: super::terminal::StdoutTerminal,
    pub state: RequestTuiState,
}

impl RequestTuiApp {
    pub fn new(meta: RequestMeta, body: &str) -> anyhow::Result<Self> {
        Ok(Self {
            terminal: super::terminal::enter()?,
            state: RequestTuiState::new(meta, body),
        })
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        super::terminal::exit(&mut self.terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(body_lines: usize, viewport: usize) -> RequestTuiState {
        let meta = RequestMeta {
            model: "gpt-4".to_string(),
            host: "api.example.com".to_string(),
            duration_ms: 1200,
            usage_tokens: Some(30),
            stream_state: StreamState::Off,
        };
        let mut state = RequestTuiState::new(meta, &"x\n".repeat(body_lines));
        state.set_viewport_height(viewport);
        state
    }

    #[test]
    fn starts_not_running_at_top() {
        let state = state_with(40, 10);
        assert!(!state.running);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn quit_ends_session() {
        let mut state = state_with(40, 10);
        state.running = true;
        state.quit();
        assert!(!state.running);
    }

    #[test]
    fn empty_body_shows_placeholder() {
        let meta = RequestMeta {
            model: "m".to_string(),
            host: "h".to_string(),
            duration_ms: 0,
            usage_tokens: None,
            stream_state: StreamState::Off,
        };
        let state = RequestTuiState::new(meta, "   ");
        assert_eq!(state.body_lines, vec!["(empty response)".to_string()]);
    }

    #[test]
    fn empty_document_scrolls_to_top() {
        let mut state = state_with(0, 10);
        state.scroll_down();
        assert_eq!(state.scroll, 0);
    }
}

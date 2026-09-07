use crate::tui::scroll::Scrollable;

/// Viewer state for the `report` TUI. The TUI renders a static Markdown
/// document produced by the pure report renderer, so the state is just a
/// scroll offset over pre-split lines plus a quit flag. No live refresh.
#[derive(Debug, Default)]
pub struct ReportTuiState {
    pub running: bool,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub viewport_height: usize,
}

impl ReportTuiState {
    pub fn new(markdown: &str) -> Self {
        Self {
            running: false,
            lines: markdown.lines().map(str::to_string).collect(),
            scroll: 0,
            viewport_height: 0,
        }
    }

    /// Named quit transition so the main loop doesn't poke the flag directly.
    pub fn quit(&mut self) {
        self.running = false;
    }
}

impl Scrollable for ReportTuiState {
    fn scroll(&self) -> usize {
        self.scroll
    }
    fn viewport_height(&self) -> usize {
        self.viewport_height
    }
    fn content_length(&self) -> usize {
        self.lines.len()
    }
    fn set_scroll(&mut self, offset: usize) {
        self.scroll = offset;
    }
    fn set_viewport(&mut self, height: usize) {
        self.viewport_height = height;
    }
}

pub struct ReportTuiApp {
    pub terminal: super::terminal::StdoutTerminal,
    pub state: ReportTuiState,
}

impl ReportTuiApp {
    pub fn new(markdown: &str) -> anyhow::Result<Self> {
        Ok(Self {
            terminal: super::terminal::enter()?,
            state: ReportTuiState::new(markdown),
        })
    }
    pub fn exit(&mut self) -> anyhow::Result<()> {
        super::terminal::exit(&mut self.terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(lines: usize, viewport: usize) -> ReportTuiState {
        let markdown: String = (0..lines)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut state = ReportTuiState::new(&markdown);
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
    fn quit_ends_the_session_and_is_idempotent() {
        let mut state = state_with(40, 10);
        state.running = true;
        state.quit();
        assert!(!state.running);
        state.quit();
        assert!(!state.running);
    }
}

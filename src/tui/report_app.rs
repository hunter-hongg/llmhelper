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

    /// Largest valid scroll offset: the first line where the viewport bottom
    /// touches the document end. The viewport is at least one line so a
    /// document always remains scrollable to its last line.
    pub fn max_scroll(&self) -> usize {
        self.lines
            .len()
            .saturating_sub(self.viewport_height.max(1))
    }

    /// Record the body area height and clamp the scroll offset into bounds.
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height;
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn scroll_down(&mut self) {
        self.scroll = (self.scroll + 1).min(self.max_scroll());
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    /// Half a viewport per page so the reader keeps context around the jump.
    pub fn page_down(&mut self) {
        self.scroll = (self.scroll + self.page_size()).min(self.max_scroll());
    }

    pub fn page_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(self.page_size());
    }

    fn page_size(&self) -> usize {
        (self.viewport_height / 2).max(1)
    }

    pub fn scroll_top(&mut self) {
        self.scroll = 0;
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    /// Named quit transition so the main loop doesn't poke the flag directly.
    pub fn quit(&mut self) {
        self.running = false;
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
        let markdown: String = (0..lines).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
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

    #[test]
    fn scroll_down_moves_one_line() {
        let mut state = state_with(40, 10);
        state.scroll_down();
        assert_eq!(state.scroll, 1);
    }

    #[test]
    fn scroll_down_clamps_at_document_end() {
        let mut state = state_with(40, 10);
        for _ in 0..100 {
            state.scroll_down();
        }
        assert_eq!(state.scroll, 30);
    }

    #[test]
    fn scroll_up_clamps_at_top() {
        let mut state = state_with(40, 10);
        state.scroll_down();
        state.scroll_up();
        state.scroll_up();
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn max_scroll_keeps_last_line_visible() {
        let state = state_with(40, 10);
        assert_eq!(state.max_scroll(), 30);
    }

    #[test]
    fn page_down_jumps_half_viewport() {
        let mut state = state_with(40, 10);
        state.page_down();
        assert_eq!(state.scroll, 5);
        state.page_down();
        assert_eq!(state.scroll, 10);
    }

    #[test]
    fn page_up_jumps_half_viewport() {
        let mut state = state_with(40, 10);
        state.scroll_bottom();
        state.page_up();
        assert_eq!(state.scroll, 25);
    }

    #[test]
    fn page_down_clamps_at_bottom() {
        let mut state = state_with(40, 10);
        for _ in 0..20 {
            state.page_down();
        }
        assert_eq!(state.scroll, 30);
    }

    #[test]
    fn scroll_bottom_and_top_reach_bounds() {
        let mut state = state_with(40, 10);
        state.scroll_bottom();
        assert_eq!(state.scroll, 30);
        state.scroll_top();
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn growing_viewport_clamps_scroll() {
        let mut state = state_with(40, 10);
        state.scroll_bottom();
        state.set_viewport_height(20);
        assert_eq!(state.scroll, 20);
    }

    #[test]
    fn empty_document_is_noop() {
        let mut state = state_with(0, 10);
        state.scroll_down();
        state.page_down();
        state.scroll_bottom();
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn zero_viewport_still_scrolls_to_last_line() {
        let mut state = state_with(40, 0);
        state.scroll_bottom();
        assert_eq!(state.scroll, 39);
    }

    #[test]
    fn viewport_height_zero_pages_by_one() {
        let mut state = state_with(40, 0);
        state.page_down();
        assert_eq!(state.scroll, 1);
    }
}

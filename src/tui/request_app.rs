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

/// Which phase of the generation the current turn is in, for the header.
/// Moves through `Idle` (awaiting the first event), `Thinking` (reasoning
/// channel receiving, answer not yet started), `Answering` (answer channel
/// receiving), and `Done` (stream settled). One-shot runs jump straight to
/// `Done` since their single event carries both channels at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GenerationState {
    #[default]
    Idle,
    Thinking,
    Answering,
    Done,
}

impl GenerationState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Thinking => "thinking",
            Self::Answering => "answering",
            Self::Done => "done",
        }
    }
}

/// Which text the request viewer shows. Drives the split between the answer
/// and the reasoning channel; `t` cycles through it at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingView {
    #[default]
    Answer,
    Both,
    ThinkingOnly,
}

impl ThinkingView {
    pub fn cycle(self) -> Self {
        match self {
            Self::Answer => Self::Both,
            Self::Both => Self::ThinkingOnly,
            Self::ThinkingOnly => Self::Answer,
        }
    }
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
    /// Generation phase of the current turn, driven by which channel the
    /// arriving events feed.
    pub gen_state: GenerationState,
    /// Last time a response event arrived; the elapsed timer is derived from
    /// this so the interactive view keeps a meaningful wall-clock reading.
    pub last_activity: Option<std::time::Instant>,
}

#[derive(Debug, Default)]
pub struct RequestTuiState {
    pub running: bool,
    pub body_lines: Vec<String>,
    pub scroll: usize,
    pub viewport_height: usize,
    pub meta: RequestMeta,
    pub follow: bool,
    /// Multi-turn mode: an input line is rendered and Enter re-sends the
    /// conversation. Off by default.
    pub interactive: bool,
    /// Text currently typed in the interactive input line.
    pub input: String,
    /// Reasoning channel, captured from `--reasoning-field` fields and split
    /// into the same line shape as the answer body.
    pub reasoning_lines: Vec<String>,
    /// Independent scroll offset for the reasoning pane.
    pub reasoning_scroll: usize,
    pub reasoning_viewport_height: usize,
    /// Whether reasoning capture is active for this invocation.
    pub capture_on: bool,
    /// Which channel(s) the viewer shows.
    pub view: ThinkingView,
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
            interactive: false,
            input: String::new(),
            reasoning_lines: Vec::new(),
            reasoning_scroll: 0,
            reasoning_viewport_height: 0,
            capture_on: false,
            view: ThinkingView::Answer,
        }
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Reset the reasoning channel at the start of a new interactive turn, so
    /// a stale chain of thought is never shown as the current one.
    pub fn reset_reasoning(&mut self) {
        self.reasoning_lines.clear();
        self.reasoning_scroll = 0;
    }

    /// Whether the reasoning pane has anything to show.
    pub fn has_reasoning(&self) -> bool {
        !self.reasoning_lines.is_empty()
    }

    /// Scroll the reasoning pane to its newest line when tail-following.
    pub fn follow_reasoning_tail(&mut self) {
        if self.follow {
            let max = self
                .reasoning_lines
                .len()
                .saturating_sub(self.reasoning_viewport_height);
            self.reasoning_scroll = max;
        }
    }

    fn reasoning_max_scroll(&self) -> usize {
        self.reasoning_lines
            .len()
            .saturating_sub(self.reasoning_viewport_height)
    }

    pub fn reasoning_scroll_down(&mut self) {
        self.reasoning_scroll = (self.reasoning_scroll + 1).min(self.reasoning_max_scroll());
    }

    pub fn reasoning_scroll_up(&mut self) {
        self.reasoning_scroll = self.reasoning_scroll.saturating_sub(1);
    }

    pub fn reasoning_page_down(&mut self) {
        self.reasoning_scroll = (self.reasoning_scroll + self.reasoning_viewport_height)
            .min(self.reasoning_max_scroll());
    }

    pub fn reasoning_page_up(&mut self) {
        self.reasoning_scroll = self
            .reasoning_scroll
            .saturating_sub(self.reasoning_viewport_height);
    }

    pub fn reasoning_scroll_top(&mut self) {
        self.reasoning_scroll = 0;
    }

    pub fn reasoning_scroll_bottom(&mut self) {
        self.reasoning_scroll = self.reasoning_max_scroll();
    }

    /// Apply a navigation action to whichever pane is focused: the reasoning
    /// pane when the view is thinking-only, the answer pane otherwise. The
    /// answer side delegates to the shared `Scrollable` implementation so the
    /// two panes cannot drift.
    fn scroll_pane(&mut self, action: ScrollAction, thinking: bool) {
        match (action, thinking) {
            (ScrollAction::Down, true) => self.reasoning_scroll_down(),
            (ScrollAction::Down, false) => self.scroll_down(),
            (ScrollAction::Up, true) => self.reasoning_scroll_up(),
            (ScrollAction::Up, false) => self.scroll_up(),
            (ScrollAction::PageDown, true) => self.reasoning_page_down(),
            (ScrollAction::PageDown, false) => self.page_down(),
            (ScrollAction::PageUp, true) => self.reasoning_page_up(),
            (ScrollAction::PageUp, false) => self.page_up(),
            (ScrollAction::Top, true) => self.reasoning_scroll_top(),
            (ScrollAction::Top, false) => self.scroll_top(),
            (ScrollAction::Bottom, true) => self.reasoning_scroll_bottom(),
            (ScrollAction::Bottom, false) => self.scroll_bottom(),
        }
    }

    /// Move the generation phase as an event arrives. Reasoning-only events
    /// put the generation in `Thinking`; answer events (including a one-shot
    /// response that carries both channels) put it in `Answering`. `Done` is
    /// set by the loop that owns the stream, since it sees the terminal.
    pub fn note_event(&mut self, content_delta: bool, reasoning_delta: bool) {
        if content_delta {
            self.meta.gen_state = GenerationState::Answering;
        } else if reasoning_delta && self.meta.gen_state != GenerationState::Answering {
            self.meta.gen_state = GenerationState::Thinking;
        }
    }
}

/// A navigation action for a request pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollAction {
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
}

/// Apply a navigation or quit key to a request TUI state, shared by the
/// one-shot stream loop, the stream TUI loop, and the interactive loop so the
/// dispatch is maintained in one place. Returns `true` when the key was
/// consumed, so an interactive loop can fall through to its own text-entry
/// handling for anything unrecognised. Scroll keys target the reasoning pane
/// when the view is thinking-only, the answer pane otherwise.
pub fn handle_request_key(state: &mut RequestTuiState, code: crossterm::event::KeyCode) -> bool {
    use crossterm::event::KeyCode;
    let thinking_focused = state.capture_on && state.view == ThinkingView::ThinkingOnly;
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            state.quit();
            true
        }
        // `t` cycles the channel view, but only outside interactive mode:
        // there it must remain a typed character, not a shortcut.
        KeyCode::Char('t') if state.capture_on && !state.interactive => {
            state.view = state.view.cycle();
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.scroll_pane(ScrollAction::Down, thinking_focused);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.scroll_pane(ScrollAction::Up, thinking_focused);
            state.follow = false;
            true
        }
        KeyCode::PageDown => {
            state.scroll_pane(ScrollAction::PageDown, thinking_focused);
            true
        }
        KeyCode::PageUp => {
            state.scroll_pane(ScrollAction::PageUp, thinking_focused);
            state.follow = false;
            true
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.scroll_pane(ScrollAction::Top, thinking_focused);
            state.follow = false;
            true
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.scroll_pane(ScrollAction::Bottom, thinking_focused);
            state.follow = true;
            true
        }
        _ => false,
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
    use crossterm::event::KeyCode;

    fn state_with(body_lines: usize, viewport: usize) -> RequestTuiState {
        let meta = RequestMeta {
            model: "gpt-4".to_string(),
            host: "api.example.com".to_string(),
            duration_ms: 1200,
            usage_tokens: Some(30),
            stream_state: StreamState::Off,
            ..Default::default()
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
            ..Default::default()
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

    /// A capture-on, one-shot viewer with both channels populated so view
    /// cycling, pane focus, and offsets can be exercised together.
    fn capture_state(
        body_lines: usize,
        reasoning_lines: usize,
        viewport: usize,
    ) -> RequestTuiState {
        let mut state = state_with(body_lines, viewport);
        state.capture_on = true;
        state.reasoning_lines = (0..reasoning_lines).map(|i| format!("r{i}")).collect();
        state.reasoning_viewport_height = viewport;
        state
    }

    #[test]
    fn t_cycles_view_answer_both_thinking_only() {
        let mut state = capture_state(40, 40, 10);
        assert_eq!(state.view, ThinkingView::Answer);
        for expected in [
            ThinkingView::Both,
            ThinkingView::ThinkingOnly,
            ThinkingView::Answer,
        ] {
            assert!(handle_request_key(&mut state, KeyCode::Char('t')));
            assert_eq!(state.view, expected);
        }
    }

    #[test]
    fn t_is_a_typed_character_in_interactive_mode() {
        let mut state = capture_state(40, 40, 10);
        state.interactive = true;
        assert!(!handle_request_key(&mut state, KeyCode::Char('t')));
        assert_eq!(state.view, ThinkingView::Answer);
    }

    #[test]
    fn thinking_only_view_scrolls_reasoning_without_moving_answer() {
        let mut state = capture_state(40, 40, 10);
        state.view = ThinkingView::ThinkingOnly;
        state.scroll = 3;
        handle_request_key(&mut state, KeyCode::Char('j'));
        assert_eq!(state.reasoning_scroll, 1);
        assert_eq!(state.scroll, 3);
    }

    #[test]
    fn answer_view_scrolls_answer_without_moving_reasoning() {
        let mut state = capture_state(40, 40, 10);
        state.reasoning_scroll = 5;
        handle_request_key(&mut state, KeyCode::Down);
        assert_eq!(state.scroll, 1);
        assert_eq!(state.reasoning_scroll, 5);
    }

    #[test]
    fn scrolling_up_anywhere_breaks_tail_follow() {
        for view in [ThinkingView::ThinkingOnly, ThinkingView::Both] {
            let mut state = capture_state(40, 40, 10);
            state.view = view;
            state.follow = true;
            handle_request_key(&mut state, KeyCode::Up);
            assert!(!state.follow, "view {view:?} should stop following on up");
        }
    }

    #[test]
    fn reset_reasoning_clears_channel_and_offset() {
        let mut state = capture_state(40, 40, 10);
        state.reasoning_scroll = 7;
        state.reset_reasoning();
        assert!(state.reasoning_lines.is_empty());
        assert_eq!(state.reasoning_scroll, 0);
    }

    #[test]
    fn content_event_moves_generation_to_answering() {
        let mut state = state_with(1, 1);
        assert_eq!(state.meta.gen_state, GenerationState::Idle);
        state.note_event(true, false);
        assert_eq!(state.meta.gen_state, GenerationState::Answering);
    }

    #[test]
    fn reasoning_event_moves_generation_from_idle_to_thinking() {
        let mut state = state_with(1, 1);
        state.note_event(false, true);
        assert_eq!(state.meta.gen_state, GenerationState::Thinking);
    }

    #[test]
    fn reasoning_event_does_not_demote_answering() {
        let mut state = state_with(1, 1);
        state.note_event(true, false);
        state.note_event(false, true);
        assert_eq!(state.meta.gen_state, GenerationState::Answering);
    }
}

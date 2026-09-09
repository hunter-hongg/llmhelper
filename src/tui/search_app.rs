use ratatui::widgets::TableState;

use crate::search::SearchHit;
use crate::source::MessageStatus;
use crate::tui::scroll::Scrollable;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SearchView {
    #[default]
    List,
    Detail,
}

/// Ranked hits plus the per-Source message counts, mirroring `SessionsData`.
#[derive(Default)]
pub struct SearchData {
    pub hits: Vec<SearchHit>,
    pub message_statuses: Vec<MessageStatus>,
}

#[derive(Default)]
pub struct SearchApp {
    pub running: bool,
    pub view: SearchView,
    pub query: String,
    pub case_sensitive: bool,
    pub role: Option<String>,
    /// `--project`/`--model`/`--since`/`--last`/`--source`/`--limit`/`--context`
    /// as one line, so a scoped search never looks unscoped on screen.
    pub filters: String,
    pub hits: Vec<SearchHit>,
    pub detail: Option<SearchHit>,
    pub message_statuses: Vec<MessageStatus>,
}

pub struct SearchTuiState {
    pub app: SearchApp,
    pub table_state: TableState,
    /// Scroll offset and line count for the detail body.
    pub scroll: usize,
    pub viewport_height: usize,
    pub detail_lines: Vec<String>,
    detail_width: usize,
}

impl SearchTuiState {
    pub const VISIBLE_ROWS: usize = 15;

    pub fn new(
        query: String,
        case_sensitive: bool,
        role: Option<String>,
        filters: String,
        data: SearchData,
    ) -> Self {
        let mut this = Self {
            app: SearchApp {
                running: false,
                view: SearchView::List,
                query,
                case_sensitive,
                role,
                filters,
                hits: data.hits,
                detail: None,
                message_statuses: data.message_statuses,
            },
            table_state: TableState::default(),
            scroll: 0,
            viewport_height: 0,
            detail_lines: Vec::new(),
            detail_width: 0,
        };
        // A non-empty list must start with something selected, otherwise
        // Enter and the arrow keys are all no-ops on first interaction.
        this.sync_list_selection(None);
        this
    }

    pub fn quit(&mut self) {
        self.app.running = false;
    }

    /// Merge freshly loaded hits into the state. The detail view is preserved
    /// when the same hit survives the re-run; otherwise it closes, matching
    /// the `sessions` refresh behaviour.
    pub fn apply_data(&mut self, data: SearchData) {
        let selected = self.table_state.selected();
        let detail = self.app.detail.take();

        self.app.hits = data.hits;
        self.app.message_statuses = data.message_statuses;
        self.sync_list_selection(selected);

        if let Some(detail) = detail {
            let Some(index) = self.app.hits.iter().position(|h| h.same_message(&detail)) else {
                self.close_detail();
                return;
            };
            self.app.detail = Some(self.app.hits[index].clone());
            self.app.view = SearchView::Detail;
            self.set_list_selection(index);
            self.set_scroll(0);
            self.rewrap_detail();
        }
    }

    pub fn open_detail(&mut self) {
        if self.app.view != SearchView::List {
            return;
        }
        let Some(selected) = self.table_state.selected() else {
            return;
        };
        let Some(hit) = self.app.hits.get(selected) else {
            return;
        };
        self.app.detail = Some(hit.clone());
        self.app.view = SearchView::Detail;
        self.set_scroll(0);
    }

    pub fn close_detail(&mut self) {
        if self.app.view != SearchView::Detail {
            return;
        }
        self.app.view = SearchView::List;
        self.app.detail = None;
    }

    pub fn select_next(&mut self) {
        if self.app.view != SearchView::List {
            return;
        }
        let count = self.app.hits.len();
        if count == 0 {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => std::cmp::min(i + 1, count - 1),
            None => 0,
        };
        self.set_list_selection(i);
    }

    pub fn select_previous(&mut self) {
        if self.app.view != SearchView::List {
            return;
        }
        let count = self.app.hits.len();
        if count == 0 {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.set_list_selection(i);
    }

    pub fn select_first(&mut self) {
        if self.app.view != SearchView::List || self.app.hits.is_empty() {
            return;
        }
        self.set_list_selection(0);
    }

    pub fn select_last(&mut self) {
        if self.app.view != SearchView::List || self.app.hits.is_empty() {
            return;
        }
        self.set_list_selection(self.app.hits.len() - 1);
    }

    fn set_list_selection(&mut self, selected: usize) {
        self.table_state.select(Some(selected));
        Self::adjust_offset(&mut self.table_state, selected);
    }

    fn adjust_offset(table_state: &mut TableState, selected: usize) {
        let visible = Self::VISIBLE_ROWS;
        let offset = table_state.offset();
        if selected >= offset + visible {
            *table_state.offset_mut() = selected - visible + 1;
        } else if selected < offset {
            *table_state.offset_mut() = selected;
        }
    }

    fn sync_list_selection(&mut self, selected: Option<usize>) {
        let count = self.app.hits.len();
        if count == 0 {
            self.table_state.select(None);
            *self.table_state.offset_mut() = 0;
            return;
        }
        let selected = selected.unwrap_or(0).min(count - 1);
        self.set_list_selection(selected);
    }

    /// Record the detail body width and re-wrap the open message when it
    /// changes, so word wrapping tracks the terminal rather than a constant.
    pub fn set_detail_width(&mut self, width: usize) {
        if width == self.detail_width {
            return;
        }
        self.detail_width = width;
        self.rewrap_detail();
    }

    fn rewrap_detail(&mut self) {
        let Some(detail) = self.app.detail.as_ref() else {
            self.detail_lines.clear();
            return;
        };
        self.detail_lines = wrap_text(&detail.text, self.detail_width);
        self.set_scroll(self.scroll.min(self.max_scroll()));
    }
}

impl Scrollable for SearchTuiState {
    fn scroll(&self) -> usize {
        self.scroll
    }
    fn viewport_height(&self) -> usize {
        self.viewport_height
    }
    fn content_length(&self) -> usize {
        self.detail_lines.len()
    }
    fn set_scroll(&mut self, offset: usize) {
        self.scroll = offset;
    }
    fn set_viewport(&mut self, height: usize) {
        self.viewport_height = height;
    }
}

/// Word-wrap `text` to `width` characters, preserving explicit newlines and
/// hard-splitting words that are longer than the width.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in raw.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{} {}", current, word)
            };
            if candidate.chars().count() <= width {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut rest = word;
            while rest.chars().count() > width {
                let take: String = rest.chars().take(width).collect();
                let advance = take.len();
                out.push(take);
                rest = &rest[advance..];
            }
            current = rest.to_string();
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

pub struct SearchTuiApp {
    pub terminal: super::terminal::StdoutTerminal,
    pub state: SearchTuiState,
}

impl SearchTuiApp {
    pub fn new(
        query: String,
        case_sensitive: bool,
        role: Option<String>,
        filters: String,
        data: SearchData,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            terminal: super::terminal::enter()?,
            state: SearchTuiState::new(query, case_sensitive, role, filters, data),
        })
    }
    pub fn exit(&mut self) -> anyhow::Result<()> {
        super::terminal::exit(&mut self.terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn hit(id: usize, text: &str, matches: usize) -> SearchHit {
        SearchHit {
            source: "claude".to_string(),
            session_id: format!("s{id}"),
            project: "/home/user/project".to_string(),
            model: Some("auto".to_string()),
            role: "assistant".to_string(),
            timestamp: Some(DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::minutes(id as i64)),
            matches,
            snippet: text.to_string(),
            text: text.to_string(),
        }
    }

    fn load_state(hits: Vec<SearchHit>) -> SearchTuiState {
        let mut state = SearchTuiState::new(
            "query".to_string(),
            false,
            None,
            String::new(),
            SearchData {
                hits,
                message_statuses: Vec::new(),
            },
        );
        state.app.running = true;
        state
    }

    #[test]
    fn enter_opens_selected_hit_detail() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);
        state.table_state.select(Some(1));

        state.open_detail();

        assert_eq!(state.app.view, SearchView::Detail);
        assert_eq!(state.app.detail.as_ref().unwrap().session_id, "s2");
    }

    #[test]
    fn new_state_selects_the_first_hit() {
        let state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);
        assert_eq!(state.table_state.selected(), Some(0));
    }

    #[test]
    fn enter_opens_the_first_hit_without_a_prior_arrow_key() {
        // Regression: the first draw must leave something selected, so Enter
        // works before the user has ever pressed an arrow key.
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);

        state.open_detail();

        assert_eq!(state.app.view, SearchView::Detail);
        assert_eq!(state.app.detail.as_ref().unwrap().session_id, "s1");
    }

    #[test]
    fn enter_is_noop_without_selection() {
        let mut state = load_state(vec![hit(1, "one", 1)]);
        state.table_state.select(None);

        state.open_detail();

        assert_eq!(state.app.view, SearchView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn enter_is_noop_with_empty_list() {
        let mut state = load_state(Vec::new());
        state.table_state.select(Some(0));

        state.open_detail();

        assert_eq!(state.app.view, SearchView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn esc_returns_to_list_preserving_selection() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);
        state.table_state.select(Some(1));
        state.open_detail();
        state.close_detail();

        assert_eq!(state.app.view, SearchView::List);
        assert!(state.app.detail.is_none());
        assert_eq!(state.table_state.selected(), Some(1));
    }

    #[test]
    fn navigation_in_detail_does_not_change_selected_hit() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);
        state.table_state.select(Some(0));
        state.open_detail();

        state.select_next();
        state.select_previous();

        assert_eq!(state.table_state.selected(), Some(0));
        assert_eq!(state.app.detail.as_ref().unwrap().session_id, "s1");
    }

    #[test]
    fn refresh_preserves_surviving_detail_and_clamps_selection() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1), hit(3, "three", 1)]);
        state.table_state.select(Some(1));
        state.open_detail();

        state.apply_data(SearchData {
            hits: vec![hit(2, "two", 3), hit(1, "one", 1)],
            message_statuses: vec![MessageStatus {
                name: "claude".to_string(),
                message_count: 2,
                error: None,
            }],
        });

        assert_eq!(state.app.view, SearchView::Detail);
        assert_eq!(state.app.detail.as_ref().unwrap().matches, 3);
        assert_eq!(state.table_state.selected(), Some(0));
        assert_eq!(state.app.message_statuses[0].message_count, 2);
    }

    #[test]
    fn refresh_closes_detail_when_selected_hit_disappears() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);
        state.table_state.select(Some(1));
        state.open_detail();

        state.apply_data(SearchData {
            hits: vec![hit(1, "one", 1)],
            message_statuses: Vec::new(),
        });

        assert_eq!(state.app.view, SearchView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn refresh_closes_detail_when_the_hit_body_changes() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1)]);
        state.table_state.select(Some(1));
        state.open_detail();

        state.apply_data(SearchData {
            hits: vec![hit(1, "one", 1), hit(2, "rewritten", 1)],
            message_statuses: Vec::new(),
        });

        assert_eq!(state.app.view, SearchView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn selection_clamps_when_the_list_shrinks() {
        let mut state = load_state(vec![hit(1, "one", 1), hit(2, "two", 1), hit(3, "three", 1)]);
        state.table_state.select(Some(2));

        state.apply_data(SearchData {
            hits: vec![hit(1, "one", 1)],
            message_statuses: Vec::new(),
        });

        assert_eq!(state.table_state.selected(), Some(0));
    }

    #[test]
    fn refresh_resets_detail_scroll_to_the_top() {
        let mut state = load_state(vec![hit(1, "line\nline\nline\nline", 1)]);
        state.table_state.select(Some(0));
        state.open_detail();
        state.set_detail_width(40);
        state.set_viewport_height(2);
        state.scroll_bottom();
        assert!(state.scroll > 0);

        // Same message, re-ranked: the detail survives and reading restarts.
        state.apply_data(SearchData {
            hits: vec![hit(1, "line\nline\nline\nline", 3)],
            message_statuses: Vec::new(),
        });

        assert_eq!(state.app.view, SearchView::Detail);
        assert_eq!(state.scroll, 0);
        assert_eq!(state.app.detail.as_ref().unwrap().matches, 3);
    }

    #[test]
    fn narrowing_the_detail_width_clamps_the_scroll() {
        let mut state = load_state(vec![hit(1, "one two three four five six seven eight", 1)]);
        state.table_state.select(Some(0));
        state.open_detail();
        state.set_detail_width(3);
        state.set_viewport_height(2);
        state.scroll_bottom();
        assert!(state.scroll > 0);

        // Widening collapses the body to a single line; the offset must
        // clamp back to the top rather than pointing past the document.
        state.set_detail_width(200);
        assert_eq!(state.scroll, 0);
        assert_eq!(state.max_scroll(), 0);
    }

    #[test]
    fn quit_is_idempotent() {
        let mut state = load_state(vec![hit(1, "one", 1)]);
        state.quit();
        state.quit();
        assert!(!state.app.running);
    }

    #[test]
    fn wrap_text_preserves_explicit_newlines() {
        assert_eq!(wrap_text("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn wrap_text_splits_long_words_on_char_boundaries() {
        let out = wrap_text("abcdefgh", 3);
        assert_eq!(out, vec!["abc", "def", "gh"]);
    }

    #[test]
    fn wrap_text_zero_width_yields_nothing() {
        assert!(wrap_text("anything", 0).is_empty());
    }

    #[test]
    fn wrap_text_reflows_when_the_width_changes() {
        let mut state = load_state(vec![hit(1, "the quick brown fox", 1)]);
        state.table_state.select(Some(0));
        state.open_detail();
        state.set_detail_width(4);
        assert!(state.detail_lines.len() > 1);
        state.set_detail_width(100);
        assert_eq!(state.detail_lines, vec!["the quick brown fox".to_string()]);
    }
}

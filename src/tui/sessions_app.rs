use crate::domain::record::Record;
use crate::source::SourceStatus;
use ratatui::widgets::TableState;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SessionsView {
    #[default]
    List,
    Detail,
}

#[derive(Default)]
pub struct SessionsData {
    pub records: Vec<Record>,
    pub source_statuses: Vec<SourceStatus>,
}

#[derive(Default)]
pub struct SessionsApp {
    pub running: bool,
    pub view: SessionsView,
    pub records: Vec<Record>,
    pub detail: Option<Record>,
    pub source_statuses: Vec<SourceStatus>,
}

pub struct SessionsTuiState {
    pub app: SessionsApp,
    pub table_state: TableState,
}

impl Default for SessionsTuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionsTuiState {
    pub fn new() -> Self {
        Self {
            app: SessionsApp::default(),
            table_state: TableState::default(),
        }
    }
}

impl SessionsTuiState {
    const VISIBLE_ROWS: usize = 15;

    pub fn apply_data(&mut self, data: SessionsData) {
        let selected = self.table_state.selected();
        let detail = self.app.detail.take();

        self.app.records = data.records;
        self.app.source_statuses = data.source_statuses;
        self.sync_list_selection(selected);

        if let Some(detail) = detail {
            let Some(index) = self
                .app
                .records
                .iter()
                .position(|r| r.source == detail.source && r.session_id == detail.session_id)
            else {
                self.app.view = SessionsView::List;
                self.app.detail = None;
                return;
            };
            self.app.detail = Some(self.app.records[index].clone());
            self.app.view = SessionsView::Detail;
            self.table_state.select(Some(index));
            Self::adjust_offset(&mut self.table_state, index);
        }
    }

    pub fn open_detail(&mut self) {
        if self.app.view != SessionsView::List {
            return;
        }
        let Some(selected) = self.table_state.selected() else {
            return;
        };
        let Some(record) = self.app.records.get(selected) else {
            return;
        };
        self.app.detail = Some(record.clone());
        self.app.view = SessionsView::Detail;
    }

    pub fn close_detail(&mut self) {
        if self.app.view != SessionsView::Detail {
            return;
        }
        self.app.view = SessionsView::List;
        self.app.detail = None;
    }

    pub fn select_next(&mut self) {
        if self.app.view != SessionsView::List {
            return;
        }
        let count = self.app.records.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::min(i + 1, count - 1),
                None => 0,
            };
            self.set_list_selection(i);
        }
    }
    pub fn select_previous(&mut self) {
        if self.app.view != SessionsView::List {
            return;
        }
        let count = self.app.records.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::max(i.saturating_sub(1), 0),
                None => 0,
            };
            self.set_list_selection(i);
        }
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
        let count = self.app.records.len();
        if count == 0 {
            self.table_state.select(None);
            *self.table_state.offset_mut() = 0;
            return;
        }
        let selected = selected.unwrap_or(0).min(count - 1);
        self.set_list_selection(selected);
    }
}

pub struct SessionsTuiApp {
    pub terminal: super::terminal::StdoutTerminal,
    pub state: SessionsTuiState,
}

impl SessionsTuiApp {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            terminal: super::terminal::enter()?,
            state: SessionsTuiState::new(),
        })
    }
    pub fn exit(&mut self) -> anyhow::Result<()> {
        super::terminal::exit(&mut self.terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use chrono::{DateTime, Utc};

    fn record(session_id: &str, cost: Option<f64>) -> Record {
        Record {
            session_id: session_id.to_string(),
            source: "claude".to_string(),
            project: "/home/user/project".to_string(),
            model: "auto".to_string(),
            agent: Some("build".to_string()),
            started_at: DateTime::<Utc>::UNIX_EPOCH,
            ended_at: None,
            tokens: TokenBreakdown {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 4,
            },
            message_count: 5,
            cost,
        }
    }

    fn load_state(records: Vec<Record>) -> SessionsTuiState {
        let mut state = SessionsTuiState::new();
        state.apply_data(SessionsData {
            records,
            source_statuses: Vec::new(),
        });
        state
    }

    #[test]
    fn enter_opens_selected_session_detail() {
        let mut state = load_state(vec![record("one", None), record("two", Some(1.0))]);
        state.table_state.select(Some(1));

        state.open_detail();

        assert_eq!(state.app.view, SessionsView::Detail);
        let detail = state.app.detail.as_ref().unwrap();
        assert_eq!(detail.session_id, "two");
        assert_eq!(detail.cost, Some(1.0));
    }

    #[test]
    fn enter_is_noop_without_selection() {
        let mut state = load_state(vec![record("one", None)]);
        state.table_state.select(None);

        state.open_detail();

        assert_eq!(state.app.view, SessionsView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn enter_is_noop_with_empty_list() {
        let mut state = load_state(Vec::new());
        state.table_state.select(Some(0));

        state.open_detail();

        assert_eq!(state.app.view, SessionsView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn enter_in_detail_is_noop() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.table_state.select(Some(0));
        state.open_detail();

        state.open_detail();

        assert_eq!(state.app.detail.as_ref().unwrap().session_id, "one");
    }

    #[test]
    fn esc_returns_to_list_preserving_selection() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.table_state.select(Some(1));
        state.open_detail();
        state.close_detail();

        assert_eq!(state.app.view, SessionsView::List);
        assert!(state.app.detail.is_none());
        assert_eq!(state.table_state.selected(), Some(1));
    }

    #[test]
    fn navigation_in_detail_does_not_change_selected_session() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.table_state.select(Some(0));
        state.open_detail();

        state.select_next();
        state.select_previous();

        assert_eq!(state.table_state.selected(), Some(0));
        assert_eq!(state.app.detail.as_ref().unwrap().session_id, "one");
    }

    #[test]
    fn refresh_preserves_surviving_detail_and_clamps_selection() {
        let mut state = load_state(vec![
            record("one", None),
            record("two", None),
            record("three", None),
        ]);
        state.table_state.select(Some(1));
        state.open_detail();

        let mut replacement = record("two", None);
        replacement.cost = Some(2.0);
        state.apply_data(SessionsData {
            records: vec![replacement, record("one", None)],
            source_statuses: vec![SourceStatus {
                name: "claude".to_string(),
                record_count: 2,
                error: None,
            }],
        });

        assert_eq!(state.app.view, SessionsView::Detail);
        assert_eq!(state.app.detail.as_ref().unwrap().cost, Some(2.0));
        assert_eq!(state.table_state.selected(), Some(0));
        assert_eq!(state.app.source_statuses[0].record_count, 2);
    }

    #[test]
    fn refresh_closes_detail_when_selected_session_disappears() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.table_state.select(Some(1));
        state.open_detail();

        state.apply_data(SessionsData {
            records: vec![record("one", None)],
            source_statuses: Vec::new(),
        });

        assert_eq!(state.app.view, SessionsView::List);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn refresh_closes_detail_when_source_changes_for_same_session_id() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.table_state.select(Some(1));
        state.open_detail();

        let mut replacement = record("two", None);
        replacement.source = "opencode".to_string();
        state.apply_data(SessionsData {
            records: vec![record("one", None), replacement],
            source_statuses: Vec::new(),
        });

        assert_eq!(state.app.view, SessionsView::List);
        assert!(state.app.detail.is_none());
    }
}

use crate::domain::record::Record;
use crate::source::SourceStatus;
use crate::tui::list_detail::{ListDetail, ViewSwitcher};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SessionsView {
    #[default]
    List,
    Detail,
}

impl ViewSwitcher for SessionsView {
    fn is_detail(&self) -> bool {
        *self == Self::Detail
    }
    fn show_list(&mut self) {
        *self = Self::List;
    }
    fn show_detail(&mut self) {
        *self = Self::Detail;
    }
}

#[derive(Default)]
pub struct SessionsData {
    pub records: Vec<Record>,
    pub source_statuses: Vec<SourceStatus>,
}

pub struct SessionsTuiState {
    pub list: ListDetail<Record, SessionsView>,
    pub source_statuses: Vec<SourceStatus>,
}

impl Default for SessionsTuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionsTuiState {
    pub fn new() -> Self {
        Self {
            list: ListDetail::new(),
            source_statuses: Vec::new(),
        }
    }

    /// Merge freshly loaded records into the state. A detail that survives the
    /// refresh (same `(source, session_id)`) stays open; otherwise it closes,
    /// matching the `search` refresh behaviour.
    pub fn apply_data(&mut self, data: SessionsData) {
        self.source_statuses = data.source_statuses;
        let _ = self.list.apply_items(data.records, |a, b| {
            a.source == b.source && a.session_id == b.session_id
        });
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
        state.list.table_state.select(Some(1));

        state.list.open_detail();

        assert_eq!(state.list.view, SessionsView::Detail);
        let detail = state.list.detail.as_ref().unwrap();
        assert_eq!(detail.session_id, "two");
        assert_eq!(detail.cost, Some(1.0));
    }

    #[test]
    fn enter_is_noop_without_selection() {
        let mut state = load_state(vec![record("one", None)]);
        state.list.table_state.select(None);

        state.list.open_detail();

        assert_eq!(state.list.view, SessionsView::List);
        assert!(state.list.detail.is_none());
    }

    #[test]
    fn enter_is_noop_with_empty_list() {
        let mut state = load_state(Vec::new());
        state.list.table_state.select(Some(0));

        state.list.open_detail();

        assert_eq!(state.list.view, SessionsView::List);
        assert!(state.list.detail.is_none());
    }

    #[test]
    fn enter_in_detail_is_noop() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.list.table_state.select(Some(0));
        state.list.open_detail();

        state.list.open_detail();

        assert_eq!(state.list.detail.as_ref().unwrap().session_id, "one");
    }

    #[test]
    fn esc_returns_to_list_preserving_selection() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.list.table_state.select(Some(1));
        state.list.open_detail();
        state.list.close_detail();

        assert_eq!(state.list.view, SessionsView::List);
        assert!(state.list.detail.is_none());
        assert_eq!(state.list.table_state.selected(), Some(1));
    }

    #[test]
    fn navigation_in_detail_does_not_change_selected_session() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.list.table_state.select(Some(0));
        state.list.open_detail();

        state.list.select_next();
        state.list.select_previous();

        assert_eq!(state.list.table_state.selected(), Some(0));
        assert_eq!(state.list.detail.as_ref().unwrap().session_id, "one");
    }

    #[test]
    fn refresh_preserves_surviving_detail_and_clamps_selection() {
        let mut state = load_state(vec![
            record("one", None),
            record("two", None),
            record("three", None),
        ]);
        state.list.table_state.select(Some(1));
        state.list.open_detail();

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

        assert_eq!(state.list.view, SessionsView::Detail);
        assert_eq!(state.list.detail.as_ref().unwrap().cost, Some(2.0));
        assert_eq!(state.list.table_state.selected(), Some(0));
        assert_eq!(state.source_statuses[0].record_count, 2);
    }

    #[test]
    fn refresh_closes_detail_when_selected_session_disappears() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.list.table_state.select(Some(1));
        state.list.open_detail();

        state.apply_data(SessionsData {
            records: vec![record("one", None)],
            source_statuses: Vec::new(),
        });

        assert_eq!(state.list.view, SessionsView::List);
        assert!(state.list.detail.is_none());
    }

    #[test]
    fn refresh_closes_detail_when_source_changes_for_same_session_id() {
        let mut state = load_state(vec![record("one", None), record("two", None)]);
        state.list.table_state.select(Some(1));
        state.list.open_detail();

        let mut replacement = record("two", None);
        replacement.source = "opencode".to_string();
        state.apply_data(SessionsData {
            records: vec![record("one", None), replacement],
            source_statuses: Vec::new(),
        });

        assert_eq!(state.list.view, SessionsView::List);
        assert!(state.list.detail.is_none());
    }
}

use ratatui::{
    backend::CrosstermBackend,
    widgets::TableState,
};
use std::io;

use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};

use crate::aggregator::AggregateResult;
use crate::domain::group::GroupBy;
use crate::domain::record::Record;
use crate::source::SourceStatus;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum View {
    #[default]
    Groups,
    Detail,
}

#[derive(Debug, Clone)]
pub struct GroupDetail {
    pub group_by: GroupBy,
    pub key: String,
    pub records: Vec<Record>,
}

#[derive(Default)]
pub struct App {
    pub running: bool,
    pub group_by: GroupBy,
    pub result: Option<AggregateResult>,
    pub records: Vec<Record>,
    pub source_statuses: Vec<SourceStatus>,
    pub view: View,
    pub detail: Option<GroupDetail>,
}

impl App {
    pub fn cycle_group(&mut self) {
        self.group_by = self.group_by.next();
    }
}

pub fn records_for_group(records: &[Record], group_by: GroupBy, key: &str) -> Vec<Record> {
    let mut matched: Vec<Record> = records
        .iter()
        .filter(|r| r.matches_group(group_by, key))
        .cloned()
        .collect();
    matched.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    matched
}

#[derive(Default)]
pub struct TuiState {
    pub app: App,
    pub table_state: TableState,
    pub detail_state: TableState,
}

impl TuiState {
    pub fn cycle_group(&mut self) {
        self.close_detail();
        self.app.cycle_group();
    }

    pub fn apply_data(
        &mut self,
        records: Vec<Record>,
        result: Option<AggregateResult>,
        source_statuses: Vec<SourceStatus>,
    ) {
        let detail = self.app.detail.take();
        let detail_selection = self.detail_state.selected().unwrap_or(0);
        self.app.records = records;
        self.app.result = result;
        self.app.source_statuses = source_statuses;
        self.sync_group_selection();

        if let Some(detail) = detail {
            let records = records_for_group(&self.app.records, detail.group_by, &detail.key);
            if records.is_empty() {
                self.app.view = View::Groups;
                self.detail_state.select(None);
            } else {
                self.app.detail = Some(GroupDetail {
                    group_by: detail.group_by,
                    key: detail.key,
                    records,
                });
                self.sync_detail_selection(detail_selection);
            }
        }
    }

    pub fn open_detail(&mut self) {
        if self.app.view != View::Groups {
            return;
        }
        let Some(selected) = self.table_state.selected() else {
            return;
        };
        let Some(group) = self.app.result.as_ref().and_then(|r| r.groups.get(selected)) else {
            return;
        };
        let records = records_for_group(&self.app.records, self.app.group_by, &group.key);
        if records.is_empty() {
            return;
        }
        self.app.detail = Some(GroupDetail {
            group_by: self.app.group_by,
            key: group.key.clone(),
            records,
        });
        self.app.view = View::Detail;
        self.detail_state.select(Some(0));
    }

    pub fn close_detail(&mut self) {
        self.app.view = View::Groups;
        self.app.detail = None;
        self.detail_state.select(None);
    }

    pub fn select_next(&mut self) {
        let (selected, count) = if self.app.view == View::Detail {
            (self.detail_state.selected(), self.app.detail.as_ref().map(|d| d.records.len()).unwrap_or(0))
        } else {
            (self.table_state.selected(), self.app.result.as_ref().map(|r| r.groups.len()).unwrap_or(0))
        };
        if count > 0 {
            let next = match selected {
                Some(i) => std::cmp::min(i + 1, count - 1),
                None => 0,
            };
            let state = if self.app.view == View::Detail { &mut self.detail_state } else { &mut self.table_state };
            state.select(Some(next));
        }
    }

    pub fn select_previous(&mut self) {
        let (selected, count) = if self.app.view == View::Detail {
            (self.detail_state.selected(), self.app.detail.as_ref().map(|d| d.records.len()).unwrap_or(0))
        } else {
            (self.table_state.selected(), self.app.result.as_ref().map(|r| r.groups.len()).unwrap_or(0))
        };
        if count > 0 {
            let previous = match selected {
                Some(i) => std::cmp::max(i.saturating_sub(1), 0),
                None => 0,
            };
            let state = if self.app.view == View::Detail { &mut self.detail_state } else { &mut self.table_state };
            state.select(Some(previous));
        }
    }

    pub fn table_state_mut(&mut self) -> &mut TableState {
        if self.app.view == View::Detail {
            &mut self.detail_state
        } else {
            &mut self.table_state
        }
    }

    fn sync_group_selection(&mut self) {
        let count = self.app.result.as_ref().map(|r| r.groups.len()).unwrap_or(0);
        if count == 0 {
            self.table_state.select(None);
            return;
        }
        let selected = self.table_state.selected().unwrap_or(0).min(count - 1);
        self.table_state.select(Some(selected));
    }

    fn sync_detail_selection(&mut self, selected: usize) {
        let count = self.app.detail.as_ref().map(|d| d.records.len()).unwrap_or(0);
        if count == 0 {
            self.detail_state.select(None);
            return;
        }
        let selected = selected.min(count - 1);
        self.detail_state.select(Some(selected));
    }
}

pub struct TerminalApp {
    pub terminal: ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    pub state: TuiState,
}

impl TerminalApp {
    pub fn new() -> anyhow::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend)?;
        Ok(Self {
            terminal,
            state: TuiState::default(),
        })
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::AggregateResult;
    use crate::domain::record::TokenBreakdown;
    use crate::filter::Filter;
    use chrono::{Duration, Utc};

    fn record(
        session_id: &str,
        source: &str,
        project: &str,
        model: &str,
        started_at: chrono::DateTime<Utc>,
        cost: Option<f64>,
    ) -> Record {
        Record {
            session_id: session_id.to_string(),
            source: source.to_string(),
            project: project.to_string(),
            model: model.to_string(),
            agent: None,
            started_at,
            ended_at: None,
            tokens: TokenBreakdown::default(),
            message_count: 1,
            cost,
        }
    }

    fn load_state(records: &[Record], group_by: GroupBy) -> TuiState {
        let result = AggregateResult::from_records(records, &Filter::none(), group_by);
        let mut state = TuiState::default();
        state.app.group_by = group_by;
        state.apply_data(records.to_vec(), Some(result), Vec::new());
        state
    }

    #[test]
    fn enter_opens_sorted_detail_for_selected_group() {
        let now = Utc::now();
        let records = vec![
            record("old", "claude", "/p", "auto", now - Duration::days(2), None),
            record("new", "claude", "/p", "auto", now, None),
        ];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(Some(0));

        state.open_detail();

        assert_eq!(state.app.view, View::Detail);
        let detail = state.app.detail.as_ref().unwrap();
        assert_eq!(detail.group_by, GroupBy::Source);
        assert_eq!(detail.key, "claude");
        assert_eq!(
            detail.records.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(),
            vec!["new", "old"]
        );
        assert_eq!(state.detail_state.selected(), Some(0));
    }

    #[test]
    fn enter_is_noop_without_selection() {
        let records = vec![record("s", "claude", "/p", "auto", Utc::now(), None)];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(None);

        state.open_detail();

        assert_eq!(state.app.view, View::Groups);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn enter_is_noop_without_result() {
        let mut state = TuiState::default();

        state.open_detail();

        assert_eq!(state.app.view, View::Groups);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn enter_is_noop_without_matching_records() {
        let records = vec![record("s", "claude", "/p", "auto", Utc::now(), None)];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(Some(0));
        state.app.records.clear();

        state.open_detail();

        assert_eq!(state.app.view, View::Groups);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn esc_returns_to_groups_preserving_group_selection() {
        let records = vec![record("s", "claude", "/p", "auto", Utc::now(), None)];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(Some(0));
        state.open_detail();
        state.close_detail();

        assert_eq!(state.app.view, View::Groups);
        assert_eq!(state.table_state.selected(), Some(0));
        assert_eq!(state.detail_state.selected(), None);
    }

    #[test]
    fn detail_matches_model_group_without_forcing_source_equality() {
        let now = Utc::now();
        let records = vec![
            record("oc", "opencode", "/p", "auto", now, Some(0.1)),
            record("claude", "claude", "/p", "auto", now - Duration::hours(1), None),
        ];
        let mut state = load_state(&records, GroupBy::Model);
        state.table_state.select(Some(0));

        state.open_detail();

        let detail = state.app.detail.as_ref().unwrap();
        assert_eq!(detail.records.len(), 2);
        assert_eq!(
            detail.records.iter().map(|r| r.cost).collect::<Vec<_>>(),
            vec![Some(0.1), None]
        );
    }

    #[test]
    fn detail_navigation_moves_within_bounds() {
        let now = Utc::now();
        let records = vec![
            record("old", "claude", "/p", "auto", now - Duration::days(2), None),
            record("new", "claude", "/p", "auto", now, None),
        ];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(Some(0));
        state.open_detail();

        state.select_next();
        state.select_next();

        assert_eq!(state.detail_state.selected(), Some(1));
    }

    #[test]
    fn refresh_clamps_detail_selection_and_returns_when_group_disappears() {
        let now = Utc::now();
        let records = vec![
            record("old", "claude", "/p", "auto", now - Duration::days(2), None),
            record("new", "claude", "/p", "auto", now, None),
        ];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(Some(0));
        state.open_detail();
        state.select_next();
        assert_eq!(state.detail_state.selected(), Some(1));

        let one = vec![record("new", "claude", "/p", "auto", now, None)];
        let one_result = AggregateResult::from_records(&one, &Filter::none(), GroupBy::Source);
        state.apply_data(one, Some(one_result), Vec::new());
        assert_eq!(state.app.view, View::Detail);
        assert_eq!(state.detail_state.selected(), Some(0));

        let empty = Vec::<Record>::new();
        let empty_result = AggregateResult::from_records(&empty, &Filter::none(), GroupBy::Source);
        state.apply_data(empty, Some(empty_result), Vec::new());
        assert_eq!(state.app.view, View::Groups);
        assert!(state.app.detail.is_none());
    }

    #[test]
    fn cycle_group_closes_detail() {
        let records = vec![record("s", "claude", "/p", "auto", Utc::now(), None)];
        let mut state = load_state(&records, GroupBy::Source);
        state.table_state.select(Some(0));
        state.open_detail();

        state.cycle_group();

        assert_eq!(state.app.view, View::Groups);
        assert_eq!(state.app.group_by, GroupBy::Project);
        assert!(state.app.detail.is_none());
    }
}

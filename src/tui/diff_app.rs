use ratatui::widgets::TableState;

use crate::aggregator::AggregateResult;
use crate::diff::DiffRow;
use crate::domain::group::GroupBy;
use crate::source::SourceStatus;

use chrono::{DateTime, Duration, Utc};

/// `(prev_start, prev_end, curr_start, curr_end, prev_duration, curr_duration)`
pub type WindowBounds<'a> = (
    &'a DateTime<Utc>,
    &'a DateTime<Utc>,
    &'a DateTime<Utc>,
    &'a DateTime<Utc>,
    &'a Duration,
    &'a Duration,
);

/// State for a diff window comparison.
#[derive(Default)]
pub struct DiffApp {
    pub running: bool,
    pub group_by: GroupBy,
    pub prev_agg: Option<AggregateResult>,
    pub curr_agg: Option<AggregateResult>,
    pub rows: Vec<DiffRow>,
    pub source_statuses: Vec<SourceStatus>,
    pub window_prev_start: Option<DateTime<Utc>>,
    pub window_prev_end: Option<DateTime<Utc>>,
    pub window_curr_start: Option<DateTime<Utc>>,
    pub window_curr_end: Option<DateTime<Utc>>,
    /// The `--last` and `--prev` durations; the window *boundaries* are
    /// recomputed on every refresh from these, so the header stays honest
    /// while time slides forward.
    pub last_duration: Option<Duration>,
    pub prev_duration: Option<Duration>,
}

impl DiffApp {
    pub fn cycle_group(&mut self) {
        self.group_by = self.group_by.next();
    }

    /// Recompute window boundaries from the stored durations at a fresh `now`,
    /// keeping the header timestamps in sync with what the filters actually
    /// compare on every refresh.
    pub fn refresh_windows(&mut self, now: DateTime<Utc>) {
        let (Some(last), Some(prev)) = (self.last_duration, self.prev_duration) else {
            return;
        };
        let prev_until = now - last;
        let curr_since = prev_until - prev;
        self.window_prev_start = Some(curr_since);
        self.window_prev_end = Some(prev_until);
        self.window_curr_start = Some(prev_until);
        self.window_curr_end = Some(now);
    }

    /// Window boundaries plus the two durations, once all are known.
    pub fn window_bounds(&self) -> Option<WindowBounds<'_>> {
        Some((
            self.window_prev_start.as_ref()?,
            self.window_prev_end.as_ref()?,
            self.window_curr_start.as_ref()?,
            self.window_curr_end.as_ref()?,
            self.prev_duration.as_ref()?,
            self.last_duration.as_ref()?,
        ))
    }

    pub fn prev_total_sessions(&self) -> usize {
        self.prev_agg
            .as_ref()
            .map(|a| a.grand_sessions)
            .unwrap_or(0)
    }

    pub fn curr_total_sessions(&self) -> usize {
        self.curr_agg
            .as_ref()
            .map(|a| a.grand_sessions)
            .unwrap_or(0)
    }
}

pub struct DiffTuiState {
    pub app: DiffApp,
    pub table_state: TableState,
}

impl Default for DiffTuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffTuiState {
    pub fn new() -> Self {
        Self {
            app: DiffApp::default(),
            table_state: TableState::default(),
        }
    }
}

impl DiffTuiState {
    pub fn select_next(&mut self) {
        let count = self.app.rows.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::min(i + 1, count - 1),
                None => 0,
            };
            self.table_state.select(Some(i));
        }
    }

    pub fn select_previous(&mut self) {
        let count = self.app.rows.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::max(i.saturating_sub(1), 0),
                None => 0,
            };
            self.table_state.select(Some(i));
        }
    }
}

pub struct DiffTuiApp {
    pub terminal: super::terminal::StdoutTerminal,
    pub state: DiffTuiState,
}

impl DiffTuiApp {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            terminal: super::terminal::enter()?,
            state: DiffTuiState::new(),
        })
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        super::terminal::exit(&mut self.terminal)
    }
}

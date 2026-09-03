use ratatui::{
    backend::CrosstermBackend,
    widgets::TableState,
};

use std::io;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::aggregator::AggregateResult;
use crate::domain::group::GroupBy;
use crate::diff::DiffRow;
use crate::source::SourceStatus;

use chrono::{DateTime, Utc};

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
}

impl DiffApp {
    pub fn cycle_group(&mut self) {
        self.group_by = self.group_by.next();
    }

    pub fn prev_total_sessions(&self) -> usize {
        self.prev_agg.as_ref().map(|a| a.grand_sessions).unwrap_or(0)
    }

    pub fn curr_total_sessions(&self) -> usize {
        self.curr_agg.as_ref().map(|a| a.grand_sessions).unwrap_or(0)
    }
}

pub struct DiffTuiState {
    pub app: DiffApp,
    pub table_state: TableState,
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
    pub terminal: ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    pub state: DiffTuiState,
}

impl DiffTuiApp {
    pub fn new() -> anyhow::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend)?;
        Ok(Self {
            terminal,
            state: DiffTuiState::new(),
        })
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

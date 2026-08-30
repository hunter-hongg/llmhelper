use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, TableState},
    Frame, Terminal,
};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;

use crate::aggregator::AggregateResult;
use crate::domain::group::GroupBy;
use crate::source::SourceStatus;

pub struct TuiState {
    pub app: App,
    pub table_state: TableState,
    pub frame_count: u64,
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            app: App::default(),
            table_state: TableState::default(),
            frame_count: 0,
        }
    }
}

#[derive(Default)]
pub struct App {
    pub running: bool,
    pub group_by: GroupBy,
    pub result: Option<AggregateResult>,
    pub source_statuses: Vec<SourceStatus>,
    pub error: Option<String>,
    pub refresh_requested: bool,
}

impl App {
    pub fn cycle_group(&mut self) {
        self.group_by = self.group_by.next();
        self.refresh_requested = true;
    }
}

impl TuiState {
    pub fn select_next(&mut self) {
        let count = self.app.result.as_ref().map(|r| r.groups.len()).unwrap_or(0);
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::min(i + 1, count - 1),
                None => 0,
            };
            self.table_state.select(Some(i));
        }
    }

    pub fn select_previous(&mut self) {
        let count = self.app.result.as_ref().map(|r| r.groups.len()).unwrap_or(0);
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::max(i.saturating_sub(1), 0),
                None => 0,
            };
            self.table_state.select(Some(i));
        }
    }
}

pub struct TerminalApp {
    pub terminal: Terminal<CrosstermBackend<io::Stdout>>,
    pub state: TuiState,
}

impl TerminalApp {
    pub fn new() -> anyhow::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            state: TuiState::new(),
        })
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

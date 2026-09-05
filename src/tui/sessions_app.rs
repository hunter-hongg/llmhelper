use ratatui::{
    backend::CrosstermBackend,
    widgets::TableState,
};
use std::io;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use crate::domain::record::Record;
use crate::source::SourceStatus;

#[derive(Default)]
pub struct SessionsApp {
    pub running: bool,
    pub records: Vec<Record>,
    pub source_statuses: Vec<SourceStatus>,
}

pub struct SessionsTuiState {
    pub app: SessionsApp,
    pub table_state: TableState,
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

    pub fn select_next(&mut self) {
        let count = self.app.records.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::min(i + 1, count - 1),
                None => 0,
            };
            self.table_state.select(Some(i));
            Self::adjust_offset(&mut self.table_state, i);
        }
    }
    pub fn select_previous(&mut self) {
        let count = self.app.records.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::max(i.saturating_sub(1), 0),
                None => 0,
            };
            self.table_state.select(Some(i));
            Self::adjust_offset(&mut self.table_state, i);
        }
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
}

pub struct SessionsTuiApp {
    pub terminal: ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    pub state: SessionsTuiState,
}

impl SessionsTuiApp {
    pub fn new() -> anyhow::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend)?;
        Ok(Self {
            terminal,
            state: SessionsTuiState::new(),
        })
    }
    pub fn exit(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

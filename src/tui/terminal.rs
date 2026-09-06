use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use std::io;

pub type StdoutTerminal = ratatui::Terminal<CrosstermBackend<io::Stdout>>;

/// Enter raw mode + alternate screen and return a terminal over stdout.
/// Shared by every subcommand TUI so setup/teardown stays in one place.
pub fn enter() -> anyhow::Result<StdoutTerminal> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(ratatui::Terminal::new(backend)?)
}

/// Leave the alternate screen, disable raw mode, and restore the cursor.
pub fn exit(terminal: &mut StdoutTerminal) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

pub mod app;
pub mod diff_app;
pub mod diff_render;
pub mod list_detail;
pub mod render;
pub mod report_app;
pub mod report_render;
pub mod request_app;
pub mod request_render;
pub mod scroll;
pub mod search_app;
pub mod search_render;
pub mod sessions_app;
pub mod sessions_render;
pub mod table;
pub mod terminal;
pub mod viewer;
pub mod watch_app;
pub mod watch_render;

/// A trait for TUI state that participates in the shared [`run_static_tui`] loop.
///
/// Implementing this on a TUI state struct lets it be driven by
/// [`run_static_tui`], which owns the draw-poll-handle boilerplate shared by
/// the simpler TUIs (`report`, `search`, `sessions`) to avoid duplicating the
/// crossterm event polling loop.
pub trait TuiLoopState {
    /// Whether the loop should keep running.
    fn is_running(&self) -> bool;
    /// Clear the running flag (called on `q`/`Esc` when the key handler didn't).
    fn quit(&mut self);
}

/// A TUI loop that drains a background channel each frame, renders, and polls
/// for keyboard input — the skeleton shared by `report`, `search`, and
/// `sessions`. The caller supplies a render closure, a key-handler closure, and
/// a channel receiver; when `rx` is `None` the loop runs without background
/// refresh (e.g. the static `report` viewer).
///
/// This replaces the per-command draw-poll-handle boilerplate that was
/// duplicated five times in `main.rs`, normalising the 200 ms poll interval.
pub fn run_static_tui<S, R, K>(
    terminal: &mut terminal::StdoutTerminal,
    state: &mut S,
    render: R,
    handle_key: K,
) -> anyhow::Result<()>
where
    S: TuiLoopState,
    R: FnMut(&mut terminal::StdoutTerminal, &mut S) -> anyhow::Result<()>,
    K: FnMut(&mut S, crossterm::event::KeyCode) -> bool,
{
    let mut render = render;
    let mut handle_key = handle_key;
    while state.is_running() {
        render(terminal, state)?;
        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if !handle_key(state, key.code) {
                    match key.code {
                        crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc => {
                            state.quit();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

pub use app::{App, TerminalApp, TuiState, View};
pub use diff_app::{DiffApp, DiffTuiApp, DiffTuiState};
pub use list_detail::{ListDetail, ViewSwitcher};
pub use report_app::{ReportTuiApp, ReportTuiState};
pub use request_app::{RequestMeta, RequestTuiApp, RequestTuiState};
pub use search_app::{SearchData, SearchTuiApp, SearchTuiState, SearchView};
pub use sessions_app::{SessionsData, SessionsTuiApp, SessionsTuiState, SessionsView};
pub use watch_app::{WatchApp, WatchTuiApp, WatchTuiState};

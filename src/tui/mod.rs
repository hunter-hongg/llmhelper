pub mod app;
pub mod render;
pub mod diff_app;
pub mod diff_render;

pub use app::{App, TuiState, TerminalApp};
pub use diff_app::{DiffApp, DiffTuiApp, DiffTuiState};

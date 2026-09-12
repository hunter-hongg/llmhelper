use ratatui::widgets::TableState;

use crate::aggregator::AggregateResult;
use crate::diff::{window_pair, DiffMode, DiffRow};
use crate::domain::group::GroupBy;
use crate::source::SourceStatus;

use chrono::{DateTime, Utc};

/// `(prev_start, prev_end, curr_start, curr_end, prev_label, curr_label)`
pub type WindowBounds<'a> = (
    &'a DateTime<Utc>,
    &'a DateTime<Utc>,
    &'a DateTime<Utc>,
    &'a DateTime<Utc>,
    &'a str,
    &'a str,
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
    /// How the windows are anchored (sliding durations or calendar buckets).
    /// The *boundaries* are recomputed on every refresh from this, so the header
    /// stays honest while time slides forward.
    pub mode: DiffMode,
    /// The `--last`/`--prev` text as typed, shown in the header in calendar mode
    /// where a bucket is a calendar keyword rather than a fixed duration.
    pub last_label: String,
    pub prev_label: String,
}

impl Default for DiffMode {
    fn default() -> Self {
        DiffMode::Sliding {
            last: chrono::Duration::zero(),
            prev: chrono::Duration::zero(),
        }
    }
}

impl DiffApp {
    pub fn cycle_group(&mut self) {
        self.group_by = self.group_by.next();
    }

    /// Recompute window boundaries from the stored mode at a fresh `now`,
    /// keeping the header timestamps in sync with what the filters actually
    /// compare on every refresh.
    pub fn refresh_windows(&mut self, now: DateTime<Utc>) {
        let Some((prev_start, prev_end, curr_start, curr_end)) = window_pair(self.mode, now) else {
            return;
        };
        self.window_prev_start = Some(prev_start);
        self.window_prev_end = Some(prev_end);
        self.window_curr_start = Some(curr_start);
        self.window_curr_end = Some(curr_end);
    }

    /// Window boundaries plus the two window labels, once all are known.
    pub fn window_bounds(&self) -> Option<WindowBounds<'_>> {
        Some((
            self.window_prev_start.as_ref()?,
            self.window_prev_end.as_ref()?,
            self.window_curr_start.as_ref()?,
            self.window_curr_end.as_ref()?,
            self.prev_label.as_str(),
            self.last_label.as_str(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        chrono::Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn calendar_refresh_yields_adjacent_local_midnight_bounds() {
        let mut app = DiffApp {
            mode: DiffMode::Calendar {
                last_days: 1,
                prev_days: 1,
            },
            ..Default::default()
        };
        app.refresh_windows(local(2026, 9, 13, 14, 32));

        assert_eq!(app.window_curr_start, Some(local(2026, 9, 13, 0, 0)));
        assert_eq!(app.window_curr_end, Some(local(2026, 9, 13, 14, 32)));
        // Adjacent: the previous window ends where the current one begins.
        assert_eq!(app.window_prev_end, app.window_curr_start);
        assert_eq!(app.window_prev_start, Some(local(2026, 9, 12, 0, 0)));
    }

    #[test]
    fn second_refresh_keeps_the_same_calendar_bucket_start() {
        let mut app = DiffApp {
            mode: DiffMode::Calendar {
                last_days: 1,
                prev_days: 1,
            },
            ..Default::default()
        };
        app.refresh_windows(local(2026, 9, 13, 14, 32));
        let first_start = app.window_curr_start;

        // A later refresh the same local day must not move the bucket start,
        // even though the window's end slides forward with `now`.
        app.refresh_windows(local(2026, 9, 13, 15, 5));
        assert_eq!(app.window_curr_start, first_start);
        assert_eq!(app.window_curr_end, Some(local(2026, 9, 13, 15, 5)));
    }

    #[test]
    fn sliding_refresh_uses_rolling_durations() {
        let mut app = DiffApp {
            mode: DiffMode::Sliding {
                last: chrono::Duration::days(7),
                prev: chrono::Duration::days(7),
            },
            ..Default::default()
        };
        let now = local(2026, 9, 13, 14, 32);
        app.refresh_windows(now);

        assert_eq!(app.window_curr_end, Some(now));
        assert_eq!(app.window_curr_start, Some(now - chrono::Duration::days(7)));
        assert_eq!(app.window_prev_end, app.window_curr_start);
        assert_eq!(
            app.window_prev_start,
            Some(now - chrono::Duration::days(14))
        );
    }
}

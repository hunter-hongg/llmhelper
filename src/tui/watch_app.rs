use chrono::{DateTime, Utc};
use ratatui::widgets::TableState;

use crate::aggregator::{AggregateResult, Group};
use crate::budget::{BudgetState, EvaluatedBudget};
use crate::domain::group::GroupBy;
use crate::domain::window::WindowMode;
use crate::source::SourceStatus;

/// State for the live monitor.
///
/// `watch` is deliberately not a browser: there is no selection, no detail
/// view and no grouping cycle, so this state carries only what a monitor
/// needs — the current frame, the frame before it, and the two timestamps
/// that let the header say what it is actually showing.
#[derive(Default)]
pub struct WatchApp {
    pub running: bool,
    pub group_by: GroupBy,
    pub result: Option<AggregateResult>,
    pub source_statuses: Vec<SourceStatus>,
    pub budgets: Vec<EvaluatedBudget>,
    /// How the window is anchored, so the header can name it and a calendar
    /// bucket can stay pinned to local midnight across reloads.
    pub mode: Option<WindowMode>,
    /// When the frame currently on screen was read.
    pub loaded_at: Option<DateTime<Utc>>,
    /// Seconds between scheduled reloads.
    pub interval_secs: u64,
    /// When the next reload is due. Derived from `loaded_at + interval_secs`.
    pub next_refresh_at: Option<DateTime<Utc>>,
    /// The frame on screen *before* the current one. Used only as the delta
    /// baseline; it is never rendered on its own.
    pub prev_agg: Option<AggregateResult>,
    /// When the previous frame was read, so the header can say how long the
    /// deltas span.
    pub prev_loaded_at: Option<DateTime<Utc>>,
}

impl WatchApp {
    /// Install a freshly loaded frame, demoting the outgoing one to the delta
    /// baseline. The first frame therefore has no baseline, and every delta
    /// reads as "not yet observed" rather than as a fabricated zero.
    pub fn apply_data(
        &mut self,
        result: Option<AggregateResult>,
        source_statuses: Vec<SourceStatus>,
        budgets: Vec<EvaluatedBudget>,
        updated_at: DateTime<Utc>,
    ) {
        self.prev_agg = self.result.take();
        self.prev_loaded_at = self.loaded_at;
        self.result = result;
        self.source_statuses = source_statuses;
        self.budgets = budgets;
        self.loaded_at = Some(updated_at);
        self.next_refresh_at =
            Some(updated_at + chrono::Duration::seconds(self.interval_secs as i64));
    }

    /// Total tokens across the canonical four-part breakdown for one group.
    ///
    /// Every bucket is summed, not just input/output: a group that moved tokens
    /// between cache and input has changed, and reporting that as "no delta"
    /// would hide real movement.
    fn group_tokens(group: &Group) -> u64 {
        group.tokens.input
            + group.tokens.output
            + group.tokens.cache_read
            + group.tokens.cache_write
    }

    fn current_group(&self, key: &str) -> Option<&Group> {
        self.result.as_ref()?.groups.iter().find(|g| g.key == key)
    }

    fn previous_group(&self, key: &str) -> Option<&Group> {
        self.prev_agg.as_ref()?.groups.iter().find(|g| g.key == key)
    }

    /// Tokens gained (or lost) since the previous frame for this group key.
    ///
    /// `None` means "not observable": there is no previous frame at all (the
    /// first read), or the current frame no longer carries this key. A group
    /// that is **new since the previous frame** reports its whole current value
    /// — that is a genuine change the monitor did observe, not an unknown.
    pub fn token_delta(&self, key: &str) -> Option<i64> {
        // Require a baseline before reporting anything: without a previous
        // frame there is nothing to subtract, and `+10` would be a claim about
        // a change that was never seen.
        self.prev_agg.as_ref()?;
        let curr = Self::group_tokens(self.current_group(key)?) as i64;
        let prev = match self.previous_group(key) {
            Some(g) => Self::group_tokens(g) as i64,
            None => 0,
        };
        Some(curr - prev)
    }

    /// Cost gained (or lost) since the previous frame for this group key.
    ///
    /// Only reported when **both** frames record a cost for the key. Cost is
    /// source-scoped (ADR 0001): a group that gained or lost its cost — for
    /// instance because a costless Claude record joined it — must show "not
    /// measured" rather than a delta that silently treats a missing value as 0.
    pub fn cost_delta(&self, key: &str) -> Option<f64> {
        let curr = self.current_group(key)?.cost?;
        let prev = self.previous_group(key)?.cost?;
        Some(curr - prev)
    }

    /// The label for the reload countdown, pure in `now`.
    pub fn countdown_text(&self, now: DateTime<Utc>) -> String {
        let Some(next) = self.next_refresh_at else {
            return String::new();
        };
        let remaining = (next - now).num_seconds();
        if remaining <= 0 {
            // The reload is already due; the timer fires on schedule, so this
            // frame is simply a little stale rather than a sign of drift.
            return "reloading…".to_string();
        }
        if remaining < 60 {
            format!("next in {}s", remaining)
        } else {
            format!("next in {}m {}s", remaining / 60, remaining % 60)
        }
    }

    /// How long the currently rendered deltas span, once a previous frame is
    /// known. `None` on the first frame — there is no span to describe.
    pub fn window_span_text(&self) -> Option<String> {
        let (prev, now) = (self.prev_loaded_at?, self.loaded_at?);
        let secs = (now - prev).num_seconds().max(0);
        Some(format!("{}s", secs))
    }

    /// The window as the user asked for it. A calendar bucket is a partial
    /// local day, so it is named by its keyword rather than by a duration that
    /// would overstate it.
    pub fn window_label(&self) -> Option<String> {
        match self.mode? {
            WindowMode::Calendar { days } => Some(format!(
                "last {} (calendar)",
                calendar_keyword(days).unwrap_or_else(|| format!("{}d", days))
            )),
            WindowMode::Rolling { last } => Some(format!("last {}", fmt_duration(last))),
        }
    }

    /// A short budget summary, worded exactly like `usage`'s so the two views
    /// cannot describe the same budgets differently. `None` when nothing is
    /// configured — the header then omits the line rather than rendering it
    /// empty.
    pub fn budget_indicator(&self) -> Option<String> {
        if self.budgets.is_empty() {
            return None;
        }
        let over = self
            .budgets
            .iter()
            .filter(|b| b.status.state == BudgetState::Over)
            .count();
        let measured = self
            .budgets
            .iter()
            .filter(|b| b.status.state != BudgetState::NotMeasured)
            .count();
        if over > 0 {
            Some(format!("budget: {} over", over))
        } else if measured == 0 {
            Some("budget: not measured".to_string())
        } else {
            Some("budget: ok".to_string())
        }
    }

    /// Whether this Source has crossed a budget, so its row is flagged.
    pub fn source_is_over_budget(&self, source: &str) -> bool {
        self.budgets
            .iter()
            .any(|b| b.status.budget.source == source && b.status.state == BudgetState::Over)
    }
}

/// Map a trailing local-day count back to the keyword the user typed, so the
/// header names the window the way it was requested.
fn calendar_keyword(days: u32) -> Option<String> {
    match days {
        1 => Some("1d".to_string()),
        7 => Some("1w".to_string()),
        30 => Some("1mo".to_string()),
        _ => None,
    }
}

/// Compact duration for a rolling window (`7d`, `4h`), matching `diff`'s
/// header labels.
fn fmt_duration(d: chrono::Duration) -> String {
    let secs = d.num_seconds();
    if secs % 86_400 == 0 {
        format!("{}d", secs / 86_400)
    } else if secs % 3_600 == 0 {
        format!("{}h", secs / 3_600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

/// State container mirroring the other TUIs: app state plus the table offset
/// used to scroll a long group list.
#[derive(Default)]
pub struct WatchTuiState {
    pub app: WatchApp,
    pub table_state: TableState,
}

impl WatchTuiState {
    pub fn apply_data(
        &mut self,
        result: Option<AggregateResult>,
        source_statuses: Vec<SourceStatus>,
        budgets: Vec<EvaluatedBudget>,
        updated_at: DateTime<Utc>,
    ) {
        let offset = self.table_state.offset();
        self.app
            .apply_data(result, source_statuses, budgets, updated_at);
        // The table has no selection, but its offset still has to survive a
        // reload: `sync_selection` would clamp both, so the offset is restored
        // against the new row count here instead.
        let count = self
            .app
            .result
            .as_ref()
            .map(|r| r.groups.len())
            .unwrap_or(0);
        *self.table_state.offset_mut() = offset.min(count.saturating_sub(1));
    }
}

pub struct WatchTuiApp {
    pub terminal: crate::tui::terminal::StdoutTerminal,
    pub state: WatchTuiState,
}

impl WatchTuiApp {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            terminal: crate::tui::terminal::enter()?,
            state: WatchTuiState::default(),
        })
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        crate::tui::terminal::exit(&mut self.terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;

    fn group(key: &str, tokens: u64, cost: Option<f64>) -> Group {
        Group {
            key: key.to_string(),
            source: key.to_string(),
            sessions: 1,
            messages: 2,
            tokens: TokenBreakdown {
                input: tokens,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            cost,
        }
    }

    fn agg(groups: Vec<Group>) -> AggregateResult {
        AggregateResult {
            grand_totals: TokenBreakdown::default(),
            grand_messages: 0,
            grand_sessions: 0,
            groups,
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn app_with(frames: &[Vec<Group>]) -> WatchApp {
        let mut app = WatchApp {
            interval_secs: 5,
            mode: None,
            ..Default::default()
        };
        for (i, groups) in frames.iter().enumerate() {
            app.apply_data(
                Some(agg(groups.clone())),
                Vec::new(),
                Vec::new(),
                at(i as i64),
            );
        }
        app
    }

    #[test]
    fn first_frame_has_no_delta_baseline() {
        let app = app_with(&[vec![group("opencode", 1000, Some(1.0))]]);
        assert!(app.token_delta("opencode").is_none());
        assert!(app.cost_delta("opencode").is_none());
        assert!(app.window_span_text().is_none());
    }

    #[test]
    fn second_frame_reports_the_signed_token_delta() {
        let app = app_with(&[
            vec![group("opencode", 1000, Some(1.0))],
            vec![group("opencode", 2500, Some(1.0))],
        ]);
        assert_eq!(app.token_delta("opencode"), Some(1500));
    }

    #[test]
    fn token_delta_sums_every_bucket_not_just_input_output() {
        let mut before = group("opencode", 0, None);
        before.tokens = TokenBreakdown {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_write: 40,
        };
        // Same total, different distribution: still a change in every bucket
        // that moved, and identical in the sum.
        let mut same_total = group("opencode", 0, None);
        same_total.tokens = TokenBreakdown {
            input: 100,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        };
        let app = app_with(&[vec![before], vec![same_total]]);
        assert_eq!(app.token_delta("opencode"), Some(0));

        let mut only_cache = group("opencode", 0, None);
        only_cache.tokens = TokenBreakdown {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_write: 55,
        };
        let app = app_with(&[vec![group("opencode", 100, None)], vec![only_cache]]);
        assert_eq!(app.token_delta("opencode"), Some(15));
    }

    #[test]
    fn a_group_new_this_frame_reports_its_whole_value() {
        let app = app_with(&[
            vec![group("opencode", 1000, Some(1.0))],
            vec![
                group("opencode", 1000, Some(1.0)),
                group("omp", 42, Some(0.5)),
            ],
        ]);
        assert_eq!(app.token_delta("omp"), Some(42));
        assert_eq!(app.cost_delta("omp"), None, "no previous cost to subtract");
    }

    #[test]
    fn a_group_gone_this_frame_has_no_delta_at_all() {
        let app = app_with(&[
            vec![group("opencode", 1000, Some(1.0))],
            vec![group("omp", 10, Some(0.1))],
        ]);
        assert!(app.token_delta("opencode").is_none());
        assert!(app.cost_delta("opencode").is_none());
    }

    #[test]
    fn cost_delta_is_reported_when_both_frames_measured_it() {
        let app = app_with(&[
            vec![group("opencode", 10, Some(1.25))],
            vec![group("opencode", 20, Some(1.50))],
        ]);
        assert_eq!(app.cost_delta("opencode"), Some(0.25));
    }

    #[test]
    fn cost_delta_is_absent_when_either_frame_lacks_cost() {
        let gained = app_with(&[
            vec![group("opencode", 10, None)],
            vec![group("opencode", 20, Some(1.0))],
        ]);
        assert!(gained.cost_delta("opencode").is_none());

        let lost = app_with(&[
            vec![group("opencode", 10, Some(1.0))],
            vec![group("opencode", 20, None)],
        ]);
        assert!(lost.cost_delta("opencode").is_none());
    }

    #[test]
    fn countdown_formats_seconds_minutes_and_the_due_boundary() {
        let mut app = WatchApp {
            interval_secs: 5,
            ..Default::default()
        };
        app.apply_data(Some(agg(vec![])), Vec::new(), Vec::new(), at(0));
        assert_eq!(app.countdown_text(at(0)), "next in 5s");
        assert_eq!(app.countdown_text(at(3)), "next in 2s");
        assert_eq!(app.countdown_text(at(5)), "reloading…");
        assert_eq!(app.countdown_text(at(9)), "reloading…");

        let mut long = WatchApp {
            interval_secs: 3600,
            ..Default::default()
        };
        long.apply_data(Some(agg(vec![])), Vec::new(), Vec::new(), at(0));
        assert_eq!(long.countdown_text(at(0)), "next in 60m 0s");
        assert_eq!(long.countdown_text(at(60)), "next in 59m 0s");
    }

    #[test]
    fn countdown_is_empty_without_a_deadline() {
        assert_eq!(WatchApp::default().countdown_text(at(0)), "");
    }

    #[test]
    fn window_span_describes_the_elapsed_time_between_frames() {
        let app = app_with(&[vec![group("a", 1, None)], vec![group("a", 2, None)]]);
        assert_eq!(app.window_span_text().as_deref(), Some("1s"));
    }

    #[test]
    fn window_label_names_calendar_keywords_and_rolling_durations() {
        let with_mode = |mode: Option<WindowMode>| WatchApp {
            mode,
            ..Default::default()
        };

        assert_eq!(
            with_mode(Some(WindowMode::Calendar { days: 1 }))
                .window_label()
                .as_deref(),
            Some("last 1d (calendar)")
        );
        assert_eq!(
            with_mode(Some(WindowMode::Calendar { days: 7 }))
                .window_label()
                .as_deref(),
            Some("last 1w (calendar)")
        );
        // A length with no keyword still names itself honestly.
        assert_eq!(
            with_mode(Some(WindowMode::Calendar { days: 3 }))
                .window_label()
                .as_deref(),
            Some("last 3d (calendar)")
        );
        assert_eq!(
            with_mode(Some(WindowMode::Rolling {
                last: chrono::Duration::hours(4),
            }))
            .window_label()
            .as_deref(),
            Some("last 4h")
        );
        assert_eq!(with_mode(None).window_label(), None);
    }

    fn evaluated(source: &str, state: BudgetState) -> EvaluatedBudget {
        use crate::budget::{Budget, BudgetWindow};
        EvaluatedBudget {
            status: crate::budget::BudgetStatus {
                budget: Budget {
                    name: format!("{}-daily", source),
                    source: source.to_string(),
                    window: BudgetWindow::parse("1d").unwrap(),
                    max_cost: 5.0,
                },
                spend: Some(6.0),
                state,
            },
            measurement: crate::budget::Measurement {
                lower_bound: None,
                clipped_by: None,
            },
        }
    }

    #[test]
    fn no_budgets_configured_produces_no_indicator() {
        assert_eq!(WatchApp::default().budget_indicator(), None);
    }

    #[test]
    fn budget_indicator_wording_matches_usage() {
        let mut app = WatchApp {
            budgets: vec![evaluated("opencode", BudgetState::Over)],
            ..Default::default()
        };
        assert_eq!(app.budget_indicator().as_deref(), Some("budget: 1 over"));
        assert!(app.source_is_over_budget("opencode"));
        assert!(!app.source_is_over_budget("omp"));

        app.budgets = vec![evaluated("opencode", BudgetState::Under)];
        assert_eq!(app.budget_indicator().as_deref(), Some("budget: ok"));
        assert!(!app.source_is_over_budget("opencode"));

        app.budgets = vec![evaluated("claude", BudgetState::NotMeasured)];
        assert_eq!(
            app.budget_indicator().as_deref(),
            Some("budget: not measured")
        );
        assert!(!app.source_is_over_budget("claude"));
    }
}

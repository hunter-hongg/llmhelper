//! Source-scoped spend budgets.
//!
//! A [`Budget`] names a ceiling on the Cost of exactly one Source over one time
//! window. Cost is a source-scoped quantity — it is never summed across
//! Sources, and Sources that record no spend (Claude Code) have no Cost at all
//! — so a budget binds to one Source and reports `NotMeasured` rather than
//! pretending absent data is zero.
//!
//! Evaluation is a pure function of `(budgets, records, now)`: it reads no
//! clock and touches no Source, so every boundary is testable with explicit
//! timestamps.

use chrono::{DateTime, Duration, Utc};

use crate::domain::record::Record;

/// The known Source names a budget may target. Kept in sync with the Source
/// registry; an unknown name is a configuration error, not a silent no-op.
pub const KNOWN_SOURCES: [&str; 5] = ["claude", "opencode", "omp", "kilo", "llmhelper"];

/// The time span a budget is measured over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetWindow {
    /// A fixed calendar bucket that resets at local midnight. `days` is the
    /// bucket length (1 = today, 7 = this week-and-prior-days, 30 = trailing
    /// 30-day block). Buckets are trailing windows ending at the current local
    /// day, not ISO weeks/months: "monthly" means the last 30 local days.
    Calendar {
        days: u32,
        /// Original text, echoed in reports (`1d`, `1w`, `1mo`).
        label: String,
    },
    /// A rolling duration measured back from `now`, using the same `N d/h/m/s`
    /// vocabulary as `--last`.
    Duration(Duration, String),
    /// Text that failed to parse. Carried so validation can report the exact
    /// bad value instead of silently substituting a window; never evaluated.
    Unparsed(String),
}

impl BudgetWindow {
    /// Human label echoed in reports.
    pub fn label(&self) -> &str {
        match self {
            BudgetWindow::Calendar { label, .. } => label,
            BudgetWindow::Duration(_, label) => label,
            BudgetWindow::Unparsed(raw) => raw,
        }
    }

    /// Parse a window, keeping unparseable text as [`BudgetWindow::Unparsed`]
    /// so later validation can name it.
    pub fn parse_or_unparsed(s: &str) -> Self {
        Self::parse(s).unwrap_or_else(|_| BudgetWindow::Unparsed(s.to_string()))
    }

    /// The inclusive lower bound of this window at `now`, or `None` for an
    /// unparsed or unbounded window.
    pub fn lower_bound(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            BudgetWindow::Duration(d, _) => Some(now - *d),
            BudgetWindow::Unparsed(_) => None,
            // Calendar buckets reset at *local* midnight, so a "daily" budget
            // means the user's day, not UTC's. The bucket math is shared with
            // `diff` via `domain::window` so the two commands cannot drift.
            BudgetWindow::Calendar { days, .. } => {
                crate::domain::window::calendar_bucket_start(now, *days)
            }
        }
    }

    /// Parse the config/CLI spelling of a window. Accepts the `--last`
    /// duration vocabulary (`30m`, `4h`, `7d`) plus the calendar keywords
    /// `1d`, `1w`, and `1mo`.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        if let Some(days) = crate::domain::window::calendar_days(s) {
            return Ok(BudgetWindow::Calendar {
                days,
                label: s.to_string(),
            });
        }
        let d = crate::cli::parse_duration(s)?;
        Ok(BudgetWindow::Duration(
            Duration::from_std(d)?,
            s.to_string(),
        ))
    }
}

/// A ceiling on one Source's Cost over one window.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    /// The name it was declared under (`[budget.<name>]`), or a synthesized
    /// name for a CLI one-off. Used for `--budget-name` and error messages.
    pub name: String,
    pub source: String,
    pub window: BudgetWindow,
    pub max_cost: f64,
}

/// Whether a Source's measured spend is under or over its ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetState {
    /// The Source records Cost and it is at or below the ceiling.
    Under,
    /// The Source records Cost and it is strictly above the ceiling.
    Over,
    /// The Source records no Cost (Claude Code), or contributed no Records.
    /// Absent data is not the same as zero spend.
    NotMeasured,
}

impl BudgetState {
    /// The machine-facing slug used in `compare`'s JSON and CSV output.
    ///
    /// Distinct from `report::budget_state_label`'s prose (`over`/`ok`/`not
    /// measured`): a JSON consumer gets an unambiguous, whitespace-free token,
    /// while the human table keeps its sentence-like label. `under` is the slug
    /// for the `Under` variant because "ok" is a rendering choice, not a name.
    pub fn json_label(&self) -> &'static str {
        match self {
            Self::Over => "over",
            Self::Under => "under",
            Self::NotMeasured => "not_measured",
        }
    }
}

/// One budget's evaluation against a record set.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetStatus {
    pub budget: Budget,
    /// Measured spend, or `None` when the Source records no Cost.
    pub spend: Option<f64>,
    pub state: BudgetState,
}

/// The lower bound actually applied when measuring a budget, reported back so
/// the renderer can tell the reader when the command's own window was narrower
/// than the budget's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    pub lower_bound: Option<DateTime<Utc>>,
    /// The command's own lower bound, when it had one and it was later than
    /// the budget's. `Some` means the budget could only see part of its window.
    pub clipped_by: Option<DateTime<Utc>>,
}

/// A [`BudgetStatus`] paired with how it was measured, ready for rendering.
/// Bundling the two keeps `render_report` from re-deriving the window, so the
/// "loaded range" caveat and the spend number always agree.
#[derive(Clone, Debug, PartialEq)]
pub struct EvaluatedBudget {
    pub status: BudgetStatus,
    pub measurement: Measurement,
}

/// Evaluate budgets and attach measurement metadata to each.
pub fn evaluate_with_measurement(
    budgets: &[Budget],
    records: &[Record],
    now: DateTime<Utc>,
    command_since: Option<DateTime<Utc>>,
) -> Vec<EvaluatedBudget> {
    budgets
        .iter()
        .map(|b| EvaluatedBudget {
            status: evaluate_one(b, records, now, command_since),
            measurement: measurement(b, now, command_since),
        })
        .collect()
}

/// Evaluate every budget against `records`.
///
/// `command_since` is the lower bound of the command's own filter (from
/// `--last`/`--since`), if any. A budget's window is independent of it, but a
/// budget can only be measured over Records the command actually loaded, so
/// the effective measurement starts at whichever bound is later — and the
/// result records that clipping so the report can say so.
pub fn evaluate(
    budgets: &[Budget],
    records: &[Record],
    now: DateTime<Utc>,
    command_since: Option<DateTime<Utc>>,
) -> Vec<BudgetStatus> {
    budgets
        .iter()
        .map(|b| evaluate_one(b, records, now, command_since))
        .collect()
}

/// The lower bound actually applied to a budget: the later of the budget's own
/// window start and the command's own filter start. Shared by evaluation and
/// measurement so the spend figure and the reported window always agree.
fn effective_lower(
    window_lower: Option<DateTime<Utc>>,
    command_since: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (window_lower, command_since) {
        (Some(w), Some(c)) => Some(w.max(c)),
        (Some(w), None) => Some(w),
        (None, c) => c,
    }
}

pub fn evaluate_one(
    budget: &Budget,
    records: &[Record],
    now: DateTime<Utc>,
    command_since: Option<DateTime<Utc>>,
) -> BudgetStatus {
    // The budget may only see records inside its own window, further bounded
    // by whatever the command already narrowed to.
    let lower = effective_lower(budget.window.lower_bound(now), command_since);
    let mut spend: Option<f64> = None;
    for r in records {
        if r.source != budget.source {
            continue;
        }
        if let Some(lo) = lower {
            if r.started_at < lo {
                continue;
            }
        }
        if let Some(c) = r.cost {
            spend = Some(spend.unwrap_or(0.0) + c);
        }
    }
    let state = match spend {
        // A Source that records Cost is judged on it, strictly greater.
        Some(s) if s > budget.max_cost => BudgetState::Over,
        Some(_) => BudgetState::Under,
        // No record carried a Cost: either the Source records none at all
        // (Claude) or none landed in the window. Either way, not zero.
        None => BudgetState::NotMeasured,
    };
    BudgetStatus {
        budget: budget.clone(),
        spend,
        state,
    }
}

/// Compute the [`Measurement`] for a budget: the bound actually used and how
/// much the command's window clipped it.
pub fn measurement(
    budget: &Budget,
    now: DateTime<Utc>,
    command_since: Option<DateTime<Utc>>,
) -> Measurement {
    let w = budget.window.lower_bound(now);
    let clipped_by = match (w, command_since) {
        (Some(w), Some(c)) if c > w => Some(c),
        _ => None,
    };
    Measurement {
        lower_bound: effective_lower(w, command_since),
        clipped_by,
    }
}

/// Evaluate `budgets` against `records` and return only the budgets whose
/// state is [`BudgetState::Over`], in the order the budgets were given.
///
/// This is the pre-flight gate for `request`: it decides whether a spend is
/// already over a ceiling, and unlike [`evaluate`] it throws away the
/// non-offenders so a caller can name the exact budgets that blocked a
/// request. Pure in its inputs, including `now` and `command_since` — it reads
/// no clock and touches no Source.
pub fn over_budget(
    budgets: &[Budget],
    records: &[Record],
    now: DateTime<Utc>,
    command_since: Option<DateTime<Utc>>,
) -> Vec<BudgetStatus> {
    evaluate(budgets, records, now, command_since)
        .into_iter()
        .filter(|s| s.state == BudgetState::Over)
        .collect()
}

/// Validate a budget's fields. Called after config load and after CLI parsing
/// so a bad ceiling fails loudly instead of silently vanishing.
pub fn validate(budget: &Budget) -> anyhow::Result<()> {
    if !KNOWN_SOURCES.contains(&budget.source.as_str()) {
        anyhow::bail!(
            "budget '{}': unknown source '{}' (known: {})",
            budget.name,
            budget.source,
            KNOWN_SOURCES.join(", ")
        );
    }
    if let BudgetWindow::Unparsed(raw) = &budget.window {
        anyhow::bail!(
            "budget '{}': invalid window '{}' (expected N d/h/m/s or 1d/1w/1mo)",
            budget.name,
            raw
        );
    }
    if !budget.max_cost.is_finite() || budget.max_cost <= 0.0 {
        anyhow::bail!(
            "budget '{}': max_cost must be a positive finite number, got {}",
            budget.name,
            budget.max_cost
        );
    }
    Ok(())
}

/// Validate every budget, naming the first offender.
pub fn validate_all(budgets: &[Budget]) -> anyhow::Result<()> {
    for b in budgets {
        validate(b)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use chrono::{Local, TimeZone};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn rec(source: &str, started_at: DateTime<Utc>, cost: Option<f64>) -> Record {
        Record {
            session_id: format!("{}_{}", source, started_at.timestamp()),
            source: source.to_string(),
            project: "/p".to_string(),
            model: "m".to_string(),
            agent: None,
            started_at,
            ended_at: None,
            tokens: TokenBreakdown::default(),
            message_count: 1,
            cost,
        }
    }

    fn budget(source: &str, window: &str, max: f64) -> Budget {
        Budget {
            name: "b".to_string(),
            source: source.to_string(),
            window: BudgetWindow::parse(window).unwrap(),
            max_cost: max,
        }
    }

    #[test]
    fn under_when_spend_is_below_ceiling() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![rec("opencode", at("2026-09-13T01:00:00Z"), Some(2.0))];
        let out = evaluate(&[budget("opencode", "1d", 5.0)], &records, now, None);
        assert_eq!(out[0].state, BudgetState::Under);
        assert_eq!(out[0].spend, Some(2.0));
    }

    #[test]
    fn at_the_boundary_is_under_not_over() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![rec("opencode", at("2026-09-13T01:00:00Z"), Some(5.0))];
        let out = evaluate(&[budget("opencode", "1d", 5.0)], &records, now, None);
        assert_eq!(out[0].state, BudgetState::Under);
    }

    #[test]
    fn over_when_strictly_above_ceiling() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![rec("opencode", at("2026-09-13T01:00:00Z"), Some(5.01))];
        let out = evaluate(&[budget("opencode", "1d", 5.0)], &records, now, None);
        assert_eq!(out[0].state, BudgetState::Over);
    }

    #[test]
    fn source_with_no_cost_is_not_measured_not_ok() {
        let now = at("2026-09-13T12:00:00Z");
        // Claude records exist but carry no cost.
        let records = vec![
            rec("claude", at("2026-09-13T01:00:00Z"), None),
            rec("claude", at("2026-09-13T02:00:00Z"), None),
        ];
        let out = evaluate(&[budget("claude", "1d", 0.01)], &records, now, None);
        assert_eq!(out[0].state, BudgetState::NotMeasured);
        assert_eq!(out[0].spend, None);
    }

    #[test]
    fn no_records_at_all_is_not_measured() {
        let now = at("2026-09-13T12:00:00Z");
        let out = evaluate(&[budget("omp", "1d", 5.0)], &[], now, None);
        assert_eq!(out[0].state, BudgetState::NotMeasured);
        assert_eq!(out[0].spend, None);
    }

    #[test]
    fn only_the_budgets_source_counts() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![
            rec("opencode", at("2026-09-13T01:00:00Z"), Some(1.0)),
            rec("omp", at("2026-09-13T01:00:00Z"), Some(100.0)),
        ];
        // A budget on opencode must not be dragged over by omp's spend.
        let out = evaluate(&[budget("opencode", "1d", 5.0)], &records, now, None);
        assert_eq!(out[0].spend, Some(1.0));
        assert_eq!(out[0].state, BudgetState::Under);
    }

    #[test]
    fn multiple_budgets_are_evaluated_independently() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![
            rec("opencode", at("2026-09-13T01:00:00Z"), Some(6.0)),
            rec("omp", at("2026-09-13T01:00:00Z"), Some(1.0)),
        ];
        let out = evaluate(
            &[budget("opencode", "1d", 5.0), budget("omp", "1d", 5.0)],
            &records,
            now,
            None,
        );
        assert_eq!(out[0].state, BudgetState::Over);
        assert_eq!(out[1].state, BudgetState::Under);
    }

    #[test]
    fn records_outside_the_window_are_excluded() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![
            rec("opencode", at("2026-09-13T01:00:00Z"), Some(2.0)),
            // 40 days ago: outside a 30d window, would push it over.
            rec("opencode", at("2026-08-04T01:00:00Z"), Some(100.0)),
        ];
        let out = evaluate(&[budget("opencode", "30d", 5.0)], &records, now, None);
        assert_eq!(out[0].spend, Some(2.0));
        assert_eq!(out[0].state, BudgetState::Under);
    }

    #[test]
    fn duration_window_lower_bound_is_inclusive() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![rec(
            "opencode",
            at("2026-09-06T12:00:00Z"), // exactly 7d ago
            Some(6.0),
        )];
        let out = evaluate(&[budget("opencode", "7d", 5.0)], &records, now, None);
        assert_eq!(out[0].state, BudgetState::Over); // inclusive at the bound
    }

    #[test]
    fn command_window_clips_a_wider_budget() {
        let now = at("2026-09-13T12:00:00Z");
        // 30d ago would be in a 30d budget but outside a --last 7d command.
        let records = vec![rec("opencode", at("2026-08-20T01:00:00Z"), Some(100.0))];
        let command_since = Some(now - Duration::days(7));
        let out = evaluate(
            &[budget("opencode", "30d", 5.0)],
            &records,
            now,
            command_since,
        );
        assert_eq!(out[0].state, BudgetState::NotMeasured);
    }

    #[test]
    fn measurement_reports_clipping_when_budget_window_is_wider() {
        let now = at("2026-09-13T12:00:00Z");
        let b = budget("opencode", "30d", 5.0);
        let command_since = Some(now - Duration::days(7));
        let m = measurement(&b, now, command_since);
        assert_eq!(m.clipped_by, command_since);
    }

    #[test]
    fn measurement_does_not_report_clipping_when_budget_is_narrower() {
        let now = at("2026-09-13T12:00:00Z");
        let b = budget("opencode", "1d", 5.0);
        let command_since = Some(now - Duration::days(30));
        let m = measurement(&b, now, command_since);
        assert_eq!(m.clipped_by, None);
    }

    #[test]
    fn calendar_day_resets_at_local_midnight() {
        // Local is UTC+8 in this environment; construct a record just before
        // local midnight of "today" and one just after.
        let now_local = Local.with_ymd_and_hms(2026, 9, 13, 12, 0, 0).unwrap();
        let now = now_local.with_timezone(&Utc);
        let just_before_midnight = Local
            .with_ymd_and_hms(2026, 9, 12, 23, 59, 0)
            .unwrap()
            .with_timezone(&Utc);
        let just_after_midnight = Local
            .with_ymd_and_hms(2026, 9, 13, 0, 1, 0)
            .unwrap()
            .with_timezone(&Utc);
        let records = vec![
            rec("opencode", just_before_midnight, Some(100.0)),
            rec("opencode", just_after_midnight, Some(1.0)),
        ];
        let out = evaluate(&[budget("opencode", "1d", 5.0)], &records, now, None);
        // Only today's (local) record counts, so spend is 1.0, not 101.0.
        assert_eq!(out[0].spend, Some(1.0));
    }

    #[test]
    fn window_parse_accepts_calendar_keywords_and_durations() {
        assert!(matches!(
            BudgetWindow::parse("1d").unwrap(),
            BudgetWindow::Calendar { days: 1, .. }
        ));
        assert!(matches!(
            BudgetWindow::parse("1w").unwrap(),
            BudgetWindow::Calendar { days: 7, .. }
        ));
        assert!(matches!(
            BudgetWindow::parse("1mo").unwrap(),
            BudgetWindow::Calendar { days: 30, .. }
        ));
        assert!(matches!(
            BudgetWindow::parse("4h").unwrap(),
            BudgetWindow::Duration(..)
        ));
        assert!(matches!(
            BudgetWindow::parse("30m").unwrap(),
            BudgetWindow::Duration(..)
        ));
    }

    #[test]
    fn window_parse_rejects_garbage() {
        assert!(BudgetWindow::parse("soon").is_err());
        assert!(BudgetWindow::parse("").is_err());
    }

    #[test]
    fn validate_rejects_unknown_source_naming_it() {
        let b = budget("gemini", "1d", 5.0);
        let err = validate(&b).unwrap_err().to_string();
        assert!(err.contains("gemini"), "{}", err);
        assert!(err.contains("gemini") && err.contains("unknown source"));
    }

    #[test]
    fn validate_rejects_non_positive_amount() {
        assert!(validate(&budget("opencode", "1d", 0.0)).is_err());
        assert!(validate(&budget("opencode", "1d", -1.0)).is_err());
    }

    #[test]
    fn validate_rejects_non_finite_amount() {
        assert!(validate(&budget("opencode", "1d", f64::NAN)).is_err());
        assert!(validate(&budget("opencode", "1d", f64::INFINITY)).is_err());
    }

    #[test]
    fn validate_accepts_a_good_budget() {
        assert!(validate(&budget("opencode", "1d", 5.0)).is_ok());
        for s in KNOWN_SOURCES {
            assert!(validate(&budget(s, "1d", 1.0)).is_ok(), "{}", s);
        }
    }

    #[test]
    fn over_budget_returns_only_the_overs_in_input_order() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![
            rec("opencode", at("2026-09-13T01:00:00Z"), Some(6.0)),
            rec("omp", at("2026-09-13T01:00:00Z"), Some(1.0)),
            rec("kilo", at("2026-09-13T01:00:00Z"), Some(7.0)),
        ];
        let budgets = vec![
            budget("opencode", "1d", 5.0), // over
            budget("omp", "1d", 5.0),      // under
            budget("kilo", "1d", 5.0),     // over
        ];
        let out = over_budget(&budgets, &records, now, None);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].budget.source, "opencode");
        assert_eq!(out[1].budget.source, "kilo");
        assert!(out.iter().all(|s| s.state == BudgetState::Over));
    }

    #[test]
    fn over_budget_is_empty_when_nothing_is_over() {
        let now = at("2026-09-13T12:00:00Z");
        let records = vec![
            rec("opencode", at("2026-09-13T01:00:00Z"), Some(1.0)),
            rec("claude", at("2026-09-13T01:00:00Z"), None), // not measured
        ];
        let budgets = vec![
            budget("opencode", "1d", 5.0), // under
            budget("claude", "1d", 5.0),   // not measured
        ];
        assert!(over_budget(&budgets, &records, now, None).is_empty());
    }

    #[test]
    fn over_budget_respects_the_command_window_clip() {
        let now = at("2026-09-13T12:00:00Z");
        // 30d ago: inside a 30d budget, but outside a --last 7d command.
        let records = vec![rec("opencode", at("2026-08-20T01:00:00Z"), Some(100.0))];
        let command_since = Some(now - Duration::days(7));
        let budgets = vec![budget("opencode", "30d", 5.0)];
        assert!(over_budget(&budgets, &records, now, command_since).is_empty());
    }
}

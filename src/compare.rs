//! Ranking groups against each other inside a single window.
//!
//! This module is **pure**: it takes the `Group`s an [`AggregateResult`] already
//! computed and derives two things that aggregate does not — an order and a
//! share — plus the optional `(others)` fold. It reads no clock and touches no
//! I/O, so the ordering, the tiebreak, the share arithmetic, and the fold are
//! all unit tests with hand-built groups and explicit expected results.
//!
//! [`AggregateResult`]: crate::aggregator::AggregateResult

use crate::aggregator::Group;
use crate::budget::{BudgetState, EvaluatedBudget};
use crate::domain::record::TokenBreakdown;
use std::collections::BTreeMap;

/// The metric a [`rank`] call orders by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    /// Sum of the four token fields (the default).
    #[default]
    Tokens,
    /// The group's cost, descending, cost-less groups last.
    Cost,
    /// Session count, descending.
    Sessions,
    /// Message count, descending.
    Messages,
}

impl SortBy {
    /// The keyword this metric is selected by on the CLI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Cost => "cost",
            Self::Sessions => "sessions",
            Self::Messages => "messages",
        }
    }
}

/// One row of a ranked view.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedGroup {
    pub key: String,
    /// 1-based position in the ranked order; the `(others)` row ranks last.
    pub rank: usize,
    /// True for the synthetic tail row, never for a real group.
    pub is_others: bool,
    pub sessions: usize,
    pub messages: usize,
    pub tokens: TokenBreakdown,
    pub cost: Option<f64>,
    /// The group's *sort metric* as a percentage of the metric's grand total.
    /// `None` when the denominator is zero (or, for cost, when the group has no
    /// cost).
    pub share_pct: Option<f64>,
    /// Token-field shares, always by token count, independent of `sort_by`.
    pub input_share_pct: Option<f64>,
    pub output_share_pct: Option<f64>,
    pub cache_read_share_pct: Option<f64>,
    pub cache_write_share_pct: Option<f64>,
    /// The budget verdict bearing on the row's own Source, or `None` when no
    /// budget applies — including every `(others)` and `mixed`-source row,
    /// which have no single Source to attribute a spend to. `rank` never sets
    /// this; it is attached afterwards by [`attach_budget_states`].
    pub budget_state: Option<BudgetState>,
}

/// The label the folded tail row carries. A real group can never use it: a
/// project path, model id, or source name is never the literal `(others)`.
pub const OTHERS_KEY: &str = "(others)";

/// A ranked view: the rows plus, parallel to them, the `Group::source` each row
/// came from (`None` for the synthetic `(others)` row). The sources ride
/// alongside so the budget annotation can map a row to its Source after the
/// sort, without `RankedGroup` having to carry a field it does not render.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedView {
    pub rows: Vec<RankedGroup>,
    pub sources: Vec<Option<String>>,
}

impl TokenBreakdown {
    /// The four-field total: what `SortBy::Tokens` ranks and shares by.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Round to one decimal place, the convention `diff` uses.
fn pct(numerator: u64, denominator: u64) -> Option<f64> {
    if denominator == 0 {
        None
    } else {
        Some(((numerator as f64 / denominator as f64) * 1000.0).round() / 10.0)
    }
}

/// The value `sort_by` orders a group by, as an `Option<f64>` so cost
/// (fractional) and the integral metrics share one comparison — and so a
/// cost-less group can sort after every group that has a cost.
fn sort_value(g: &Group, sort_by: SortBy) -> Option<f64> {
    match sort_by {
        SortBy::Tokens => Some(g.tokens.total() as f64),
        SortBy::Cost => g.cost,
        SortBy::Sessions => Some(g.sessions as f64),
        SortBy::Messages => Some(g.messages as f64),
    }
}

/// The grand totals each share is measured against: token field totals, the sum
/// of cost-bearing groups' cost, session count, and message count.
struct GrandTotals {
    tokens: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
    sessions: u64,
    messages: u64,
}

impl GrandTotals {
    fn of(groups: &[Group], grand_tokens: u64) -> Self {
        Self {
            tokens: grand_tokens,
            input: groups.iter().map(|g| g.tokens.input).sum(),
            output: groups.iter().map(|g| g.tokens.output).sum(),
            cache_read: groups.iter().map(|g| g.tokens.cache_read).sum(),
            cache_write: groups.iter().map(|g| g.tokens.cache_write).sum(),
            // Only cost-bearing groups count: phantom zeros would understate
            // every real group's share.
            cost: groups.iter().filter_map(|g| g.cost).sum(),
            sessions: groups.iter().map(|g| g.sessions as u64).sum(),
            messages: groups.iter().map(|g| g.messages as u64).sum(),
        }
    }

    /// The group's sort-metric share of the matching grand total.
    fn share_of(&self, g: &Group, sort_by: SortBy) -> Option<f64> {
        match sort_by {
            SortBy::Tokens => pct(g.tokens.total(), self.tokens),
            SortBy::Cost => g.cost.map(|c| {
                if self.cost == 0.0 {
                    0.0
                } else {
                    ((c / self.cost) * 1000.0).round() / 10.0
                }
            }),
            SortBy::Sessions => pct(g.sessions as u64, self.sessions),
            SortBy::Messages => pct(g.messages as u64, self.messages),
        }
    }
}

/// Rank `groups` by `sort_by`, descending, compute each group's shares, and fold
/// the tail into an `(others)` row when `top` limits the count.
///
/// `grand_tokens` is the aggregate's own four-field grand total, passed in
/// rather than recomputed so the share denominator cannot drift from what the
/// aggregate reports.
pub fn rank(
    groups: &[Group],
    grand_tokens: u64,
    sort_by: SortBy,
    top: Option<usize>,
) -> Vec<RankedGroup> {
    rank_with_sources(groups, grand_tokens, sort_by, top).rows
}

/// Rank `groups` and also return, parallel to the rows, the `Group::source`
/// each row came from (`None` for the synthetic `(others)` row).
///
/// The budget annotation (spec 0024) needs the Source of each row *after* the
/// sort, and `RankedGroup` deliberately does not carry it — a ranked row is a
/// presentation of a group, not the group. Returning the parallel vector here
/// keeps [`rank`]'s own signature and contract intact for every existing caller
/// and test.
pub fn rank_with_sources(
    groups: &[Group],
    grand_tokens: u64,
    sort_by: SortBy,
    top: Option<usize>,
) -> RankedView {
    // Sort descending by the metric; ties broken by key ascending so the order
    // is total and the output is byte-stable across runs.
    let mut ordered: Vec<&Group> = groups.iter().collect();
    ordered.sort_by(|a, b| {
        let (av, bv) = (sort_value(a, sort_by), sort_value(b, sort_by));
        let ord = match (av, bv) {
            (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
            // A `None` (no cost) is lower than every `Some`, so it sorts last.
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        ord.then_with(|| a.key.cmp(&b.key))
    });

    let grand = GrandTotals::of(groups, grand_tokens);

    let row_for = |g: &Group, rank: usize, is_others: bool| RankedGroup {
        key: g.key.clone(),
        rank,
        is_others,
        sessions: g.sessions,
        messages: g.messages,
        tokens: g.tokens.clone(),
        cost: g.cost,
        share_pct: grand.share_of(g, sort_by),
        input_share_pct: pct(g.tokens.input, grand.input),
        output_share_pct: pct(g.tokens.output, grand.output),
        cache_read_share_pct: pct(g.tokens.cache_read, grand.cache_read),
        cache_write_share_pct: pct(g.tokens.cache_write, grand.cache_write),
        // Attached later: `rank` is budget-free by contract.
        budget_state: None,
    };

    // `top = Some(0)` means "no limit": folding everything into one row would be
    // legal but useless, and rejecting `0` would be a surprising error. A limit
    // at or above the group count folds nothing either.
    let fold_from = match top {
        Some(n) if n > 0 && n < ordered.len() => Some(n),
        _ => None,
    };

    let Some(n) = fold_from else {
        let rows: Vec<RankedGroup> = ordered
            .iter()
            .enumerate()
            .map(|(i, g)| row_for(g, i + 1, false))
            .collect();
        let sources = ordered.iter().map(|g| Some(g.source.clone())).collect();
        return RankedView { rows, sources };
    };

    let mut rows: Vec<RankedGroup> = ordered[..n]
        .iter()
        .enumerate()
        .map(|(i, g)| row_for(g, i + 1, false))
        .collect();

    // Fold the tail: sum the values, and sum the metric shares *the same way the
    // real rows computed theirs*, so the visible shares still total ~100%.
    let tail = &ordered[n..];
    let mut sessions = 0usize;
    let mut messages = 0usize;
    let mut tokens = TokenBreakdown::default();
    let mut cost: Option<f64> = Some(0.0);
    let mut share_sum = 0.0f64;
    let mut any_share = false;
    for g in tail.iter().copied() {
        sessions += g.sessions;
        messages += g.messages;
        tokens.add(&g.tokens);
        cost = match (cost, g.cost) {
            (Some(acc), Some(c)) => Some(acc + c),
            _ => None,
        };
        if let Some(s) = grand.share_of(g, sort_by) {
            share_sum += s;
            any_share = true;
        }
    }

    let others = RankedGroup {
        key: OTHERS_KEY.to_string(),
        rank: n + 1,
        is_others: true,
        sessions,
        messages,
        tokens: tokens.clone(),
        cost,
        share_pct: if any_share {
            Some((share_sum * 10.0).round() / 10.0)
        } else {
            None
        },
        input_share_pct: pct(tokens.input, grand.input),
        output_share_pct: pct(tokens.output, grand.output),
        cache_read_share_pct: pct(tokens.cache_read, grand.cache_read),
        cache_write_share_pct: pct(tokens.cache_write, grand.cache_write),
        // The `(others)` row is synthetic: it is not a Source, so no budget
        // can bear on it.
        budget_state: None,
    };
    rows.push(others);
    // The folded tail has no single Source, so the `(others)` row's source slot
    // is `None` — a budget can never mark it.
    let mut sources: Vec<Option<String>> = ordered[..n]
        .iter()
        .map(|g| Some(g.source.clone()))
        .collect();
    sources.push(None);
    RankedView { rows, sources }
}

/// Attach each row's budget verdict, in place.
///
/// `sources` is parallel to `rows`: the `Group::source` value (`"mixed"` for a
/// cross-source group) that produced each ranked row, or `None` for the
/// synthetic `(others)` row. `states` maps a Source name to its evaluated
/// budget state.
///
/// A row is marked only when its group has exactly one Source that a budget is
/// scoped to. This is the load-bearing mapping of spec 0024: a budget is
/// Source-scoped (ADR 0001), so a `mixed`-source group — which may span several
/// budgets and whose cost is permanently `None` — is never attributed a single
/// Source's verdict.
pub fn attach_budget_states(
    rows: &mut [RankedGroup],
    sources: &[Option<String>],
    states: &BTreeMap<String, BudgetState>,
) {
    for (row, source) in rows.iter_mut().zip(sources.iter()) {
        row.budget_state = source.as_deref().and_then(|s| states.get(s).copied());
    }
}

/// The Source→budget-state map consumed by [`attach_budget_states`].
/// `mixed` can never appear here — no budget is scoped to it — so a
/// cross-source row resolves to `None` automatically.
pub fn budget_states(budgets: &[EvaluatedBudget]) -> BTreeMap<String, BudgetState> {
    budgets
        .iter()
        .map(|b| (b.status.budget.source.clone(), b.status.state))
        .collect()
}

/// A one-line summary of the evaluated budgets, for the header/JSON.
///
/// `None` when no budgets are configured — the marker that a run must stay
/// byte-identical to a pre-spec-0024 `compare`. Otherwise it carries the counts
/// the header needs, so the wording lives in one place and is unit-tested,
/// mirroring `usage`'s `budget_indicator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetSummary {
    /// How many budgets are strictly over their ceiling.
    pub over: usize,
    /// How many budgets are `Under` (measured and fine).
    pub under: usize,
    /// How many budgets are `NotMeasured` (the Source records no cost).
    pub not_measured: usize,
}

impl BudgetSummary {
    /// Summarize evaluated budgets, or `None` when there are none.
    pub fn of(budgets: &[EvaluatedBudget]) -> Option<Self> {
        if budgets.is_empty() {
            return None;
        }
        let mut summary = Self {
            over: 0,
            under: 0,
            not_measured: 0,
        };
        for b in budgets {
            match b.status.state {
                BudgetState::Over => summary.over += 1,
                BudgetState::Under => summary.under += 1,
                BudgetState::NotMeasured => summary.not_measured += 1,
            }
        }
        Some(summary)
    }

    /// The header phrase, reusing the wording `usage`'s indicator fixed:
    /// `over` wins over `ok` (the count that matters), and an all-unmeasured
    /// set says so rather than claiming `ok`.
    pub fn label(&self) -> String {
        if self.over > 0 {
            format!("budgets: {} over", self.over)
        } else if self.under == 0 {
            "budgets: not measured".to_string()
        } else {
            "budgets: ok".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(
        key: &str,
        sessions: usize,
        messages: usize,
        tokens: [u64; 4],
        cost: Option<f64>,
    ) -> Group {
        Group {
            key: key.to_string(),
            source: "opencode".to_string(),
            sessions,
            messages,
            tokens: TokenBreakdown {
                input: tokens[0],
                output: tokens[1],
                cache_read: tokens[2],
                cache_write: tokens[3],
            },
            cost,
        }
    }

    fn keys(rows: &[RankedGroup]) -> Vec<String> {
        rows.iter().map(|r| r.key.clone()).collect()
    }

    #[test]
    fn tokens_sort_is_descending() {
        let groups = vec![
            group("small", 1, 1, [100, 0, 0, 0], None),
            group("big", 1, 1, [1000, 0, 0, 0], None),
            group("mid", 1, 1, [500, 0, 0, 0], None),
        ];
        let rows = rank(&groups, 1600, SortBy::Tokens, None);
        assert_eq!(keys(&rows), vec!["big", "mid", "small"]);
        assert_eq!(rows[0].rank, 1);
        assert_eq!(rows[2].rank, 3);
    }

    #[test]
    fn ties_break_by_key_and_are_deterministic() {
        let groups = vec![
            group("b", 1, 1, [100, 0, 0, 0], None),
            group("a", 1, 1, [100, 0, 0, 0], None),
            group("c", 1, 1, [100, 0, 0, 0], None),
        ];
        let first = rank(&groups, 300, SortBy::Tokens, None);
        let second = rank(&groups, 300, SortBy::Tokens, None);
        assert_eq!(keys(&first), vec!["a", "b", "c"]);
        assert_eq!(keys(&first), keys(&second));
    }

    #[test]
    fn cost_sort_puts_costless_groups_last() {
        let groups = vec![
            group("claude", 1, 1, [10, 0, 0, 0], None),
            group("cheap", 1, 1, [10, 0, 0, 0], Some(1.0)),
            group("pricey", 1, 1, [10, 0, 0, 0], Some(9.0)),
        ];
        let rows = rank(&groups, 30, SortBy::Cost, None);
        assert_eq!(keys(&rows), vec!["pricey", "cheap", "claude"]);
        assert!(rows[0].cost.is_some());
        assert_eq!(rows[2].cost, None);
        // Share of the cost-bearing total: 9 / 10 and 1 / 10.
        assert_eq!(rows[0].share_pct, Some(90.0));
        assert_eq!(rows[1].share_pct, Some(10.0));
        // A cost-less group has no cost share, never 0%.
        assert_eq!(rows[2].share_pct, None);
    }

    #[test]
    fn token_shares_are_independent_of_sort_key() {
        let groups = vec![
            group("a", 1, 1, [800, 0, 0, 0], Some(1.0)),
            group("b", 1, 1, [200, 0, 0, 0], Some(1.0)),
        ];
        let by_tokens = rank(&groups, 1000, SortBy::Tokens, None);
        let by_cost = rank(&groups, 1000, SortBy::Cost, None);
        assert_eq!(by_tokens[0].input_share_pct, Some(80.0));
        // Same input share even though the table is ordered by cost.
        assert_eq!(by_cost[0].input_share_pct, Some(80.0));
    }

    #[test]
    fn shares_sum_to_about_100() {
        let groups = vec![
            group("a", 1, 1, [333, 0, 0, 0], None),
            group("b", 1, 1, [333, 0, 0, 0], None),
            group("c", 1, 1, [334, 0, 0, 0], None),
        ];
        let rows = rank(&groups, 1000, SortBy::Tokens, None);
        let sum: f64 = rows.iter().filter_map(|r| r.share_pct).sum();
        assert!((sum - 100.0).abs() < 0.2, "shares summed to {sum}");
    }

    #[test]
    fn share_is_none_when_denominator_is_zero() {
        let groups = vec![group("a", 0, 0, [0, 0, 0, 0], None)];
        let rows = rank(&groups, 0, SortBy::Tokens, None);
        assert_eq!(rows[0].share_pct, None);
        assert_eq!(rows[0].input_share_pct, None);
    }

    #[test]
    fn others_fold_sums_values_and_shares_and_lands_last() {
        let groups = vec![
            group("a", 1, 2, [600, 0, 0, 0], None),
            group("b", 1, 3, [200, 0, 0, 0], None),
            group("c", 1, 4, [150, 0, 0, 0], None),
            group("d", 1, 5, [50, 0, 0, 0], None),
        ];
        let rows = rank(&groups, 1000, SortBy::Tokens, Some(2));
        assert_eq!(rows.len(), 3);
        assert_eq!(keys(&rows), vec!["a", "b", OTHERS_KEY]);
        let others = &rows[2];
        assert!(others.is_others);
        // c + d folded.
        assert_eq!(others.sessions, 2);
        assert_eq!(others.messages, 9);
        assert_eq!(others.tokens.total(), 200);
        assert_eq!(others.share_pct, Some(20.0));
        // Visible shares still total 100.
        let sum: f64 = rows.iter().filter_map(|r| r.share_pct).sum();
        assert!((sum - 100.0).abs() < 0.2, "shares summed to {sum}");
    }

    #[test]
    fn others_cost_sums_only_when_all_folded_are_cost_bearing() {
        let all_costing = vec![
            group("a", 1, 1, [100, 0, 0, 0], Some(5.0)),
            group("b", 1, 1, [100, 0, 0, 0], Some(3.0)),
            group("c", 1, 1, [100, 0, 0, 0], Some(2.0)),
        ];
        let rows = rank(&all_costing, 300, SortBy::Cost, Some(1));
        assert_eq!(rows[1].cost, Some(5.0));

        let mixed = vec![
            group("a", 1, 1, [100, 0, 0, 0], Some(9.0)),
            group("b", 1, 1, [100, 0, 0, 0], Some(5.0)),
            group("c", 1, 1, [100, 0, 0, 0], None),
        ];
        let rows = rank(&mixed, 300, SortBy::Cost, Some(1));
        assert_eq!(rows[1].key, OTHERS_KEY);
        assert_eq!(rows[1].cost, None);
    }

    #[test]
    fn top_zero_and_top_at_or_above_count_emit_no_others_row() {
        let groups = vec![
            group("a", 1, 1, [100, 0, 0, 0], None),
            group("b", 1, 1, [50, 0, 0, 0], None),
        ];
        let zero = rank(&groups, 150, SortBy::Tokens, Some(0));
        assert_eq!(keys(&zero), vec!["a", "b"]);
        assert!(zero.iter().all(|r| !r.is_others));

        let exact = rank(&groups, 150, SortBy::Tokens, Some(2));
        assert_eq!(keys(&exact), vec!["a", "b"]);

        let over = rank(&groups, 150, SortBy::Tokens, Some(9));
        assert_eq!(keys(&over), vec!["a", "b"]);
    }

    #[test]
    fn sort_by_sessions_and_messages_work() {
        let groups = vec![
            group("a", 1, 99, [100, 0, 0, 0], None),
            group("b", 9, 1, [100, 0, 0, 0], None),
        ];
        let by_sessions = rank(&groups, 200, SortBy::Sessions, None);
        assert_eq!(keys(&by_sessions), vec!["b", "a"]);
        let by_messages = rank(&groups, 200, SortBy::Messages, None);
        assert_eq!(keys(&by_messages), vec!["a", "b"]);
    }

    // --- Budget annotation (spec 0024) ---

    fn evaluated(source: &str, state: BudgetState) -> EvaluatedBudget {
        EvaluatedBudget {
            status: crate::budget::BudgetStatus {
                budget: crate::budget::Budget {
                    name: format!("b:{source}"),
                    source: source.to_string(),
                    window: crate::budget::BudgetWindow::parse("7d").unwrap(),
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
    fn budget_states_maps_every_source_to_its_state() {
        let budgets = vec![
            evaluated("opencode", BudgetState::Over),
            evaluated("omp", BudgetState::Under),
            evaluated("claude", BudgetState::NotMeasured),
        ];
        let states = budget_states(&budgets);
        assert_eq!(states.get("opencode"), Some(&BudgetState::Over));
        assert_eq!(states.get("omp"), Some(&BudgetState::Under));
        assert_eq!(states.get("claude"), Some(&BudgetState::NotMeasured));
        // A Source with no budget is simply absent, so its rows stay unmarked.
        assert_eq!(states.get("kilo"), None);
    }

    #[test]
    fn attach_marks_a_single_source_row_with_its_verdict() {
        let mut a = group("opencode", 1, 1, [100, 0, 0, 0], Some(6.0));
        a.source = "opencode".to_string();
        let mut b = group("omp", 1, 1, [50, 0, 0, 0], Some(1.0));
        b.source = "omp".to_string();
        let groups = vec![a, b];
        let mut view = rank_with_sources(&groups, 150, SortBy::Tokens, None);
        let states = budget_states(&[
            evaluated("opencode", BudgetState::Over),
            evaluated("omp", BudgetState::Under),
        ]);
        attach_budget_states(&mut view.rows, &view.sources, &states);
        assert_eq!(view.rows[0].budget_state, Some(BudgetState::Over));
        assert_eq!(view.rows[1].budget_state, Some(BudgetState::Under));
    }

    #[test]
    fn attach_leaves_a_mixed_source_row_unmarked() {
        // The `group` helper hardcodes source "opencode" for single-source
        // rows; build a mixed one explicitly (the aggregator's "mixed" marker).
        let mut mixed = group("shared-project", 2, 2, [200, 0, 0, 0], None);
        mixed.source = "mixed".to_string();
        let groups = vec![mixed];
        let mut view = rank_with_sources(&groups, 200, SortBy::Tokens, None);
        let states = budget_states(&[evaluated("opencode", BudgetState::Over)]);
        attach_budget_states(&mut view.rows, &view.sources, &states);
        // A Source-scoped budget cannot be attributed to a group spanning
        // Sources (ADR 0001), even though the budget's Source is present.
        assert_eq!(view.rows[0].budget_state, None);
    }

    #[test]
    fn attach_leaves_the_others_row_unmarked_and_stays_parallel() {
        let groups = vec![
            group("a", 1, 1, [1000, 0, 0, 0], Some(9.0)),
            group("b", 1, 1, [100, 0, 0, 0], Some(1.0)),
        ];
        let mut view = rank_with_sources(&groups, 1100, SortBy::Tokens, Some(1));
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.sources.len(), 2);
        assert_eq!(view.sources[1], None);
        let states = budget_states(&[evaluated("opencode", BudgetState::Over)]);
        attach_budget_states(&mut view.rows, &view.sources, &states);
        assert_eq!(view.rows[0].budget_state, Some(BudgetState::Over));
        assert!(view.rows[1].is_others);
        assert_eq!(view.rows[1].budget_state, None);
    }

    #[test]
    fn rank_is_budget_free_so_existing_callers_see_no_verdict() {
        let groups = vec![group("opencode", 1, 1, [100, 0, 0, 0], Some(6.0))];
        // `rank` returns rows with `budget_state == None` by construction.
        let rows = rank(&groups, 100, SortBy::Tokens, None);
        assert_eq!(rows[0].budget_state, None);
    }

    #[test]
    fn budget_summary_is_none_without_budgets_and_labels_over_wins() {
        assert_eq!(BudgetSummary::of(&[]), None);

        let over = BudgetSummary::of(&[
            evaluated("opencode", BudgetState::Over),
            evaluated("omp", BudgetState::Under),
        ])
        .unwrap();
        assert_eq!(over.label(), "budgets: 1 over");

        let all_under = BudgetSummary::of(&[evaluated("omp", BudgetState::Under)]).unwrap();
        assert_eq!(all_under.label(), "budgets: ok");

        let none_measured =
            BudgetSummary::of(&[evaluated("claude", BudgetState::NotMeasured)]).unwrap();
        assert_eq!(none_measured.label(), "budgets: not measured");
    }
}

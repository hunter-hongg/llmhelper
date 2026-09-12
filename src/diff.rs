use std::collections::BTreeSet;

use serde::Serialize;

use crate::aggregator::{AggregateResult, Group};
use crate::domain::record::TokenBreakdown;

/// How the two comparison windows are anchored in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffMode {
    /// Two rolling windows relative to `now`: current `[now-last, now]`,
    /// previous `[now-last-prev, now-last]`.
    Sliding {
        last: chrono::Duration,
        prev: chrono::Duration,
    },
    /// Two adjacent calendar buckets anchored to local midnight: the current
    /// bucket runs from its start to `now`, and the previous bucket is the
    /// `prev_days`-long block immediately before it.
    Calendar { last_days: u32, prev_days: u32 },
}

/// The two half-open window bounds for a comparison:
/// `(prev_since, prev_until, curr_since, curr_until)`.
pub type WindowPair = (
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
);

/// The two half-open window bounds `(prev_since, prev_until, curr_since,
/// curr_until)` for `mode` at `now`. Pure: reads no clock, so it is fully
/// testable with explicit instants. Returns `None` only when a calendar bucket
/// cannot be anchored (an impossible local midnight, e.g. a DST gap with no
/// valid instant).
pub fn window_pair(mode: DiffMode, now: chrono::DateTime<chrono::Utc>) -> Option<WindowPair> {
    match mode {
        DiffMode::Sliding { last, prev } => {
            let prev_until = now - last;
            let curr_since = prev_until - prev;
            Some((curr_since, prev_until, prev_until, now))
        }
        DiffMode::Calendar {
            last_days,
            prev_days,
        } => {
            let curr_since = crate::domain::window::calendar_bucket_start(now, last_days)?;
            // The previous bucket is exactly `prev_days` local days wide and ends
            // where the current bucket begins. Stepping back in local-day units
            // (via the shared helper) keeps the boundary on local midnight even
            // across a DST transition.
            let prev_since = crate::domain::window::calendar_bucket_before(curr_since, prev_days)?;
            Some((prev_since, curr_since, curr_since, now))
        }
    }
}

/// Human-readable window length for sliding windows: `"1d"`, `"12h"`, `"45m"`
/// (whichever is whole, most significant first). Calendar windows carry their
/// keyword (`1d`/`1w`/`1mo`) instead, since their current bucket is partial.
pub fn fmt_window_len(d: chrono::Duration) -> String {
    let s = d.num_seconds();
    let days = s / 86_400;
    let hours = (s % 86_400) / 3_600;
    let mins = (s % 3_600) / 60;
    if days > 0 && hours == 0 && mins == 0 {
        format!("{}d", days)
    } else if hours > 0 && mins == 0 {
        format!("{}h", hours)
    } else if days > 0 {
        format!("{}d {}h", days, hours)
    } else if mins > 0 {
        format!("{}m", mins)
    } else {
        format!("{}h", hours)
    }
}

/// Which windows a Group key appears in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Both,
    New,
    Removed,
}

impl std::fmt::Display for Presence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Both => write!(f, "both"),
            Self::New => write!(f, "new"),
            Self::Removed => write!(f, "removed"),
        }
    }
}

/// A single window's aggregated snapshot for one Group key.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub sessions: usize,
    pub messages: usize,
    pub tokens: TokenBreakdown,
    pub cost: Option<f64>,
}

impl From<&Group> for Snapshot {
    fn from(g: &Group) -> Self {
        Self {
            sessions: g.sessions,
            messages: g.messages,
            tokens: g.tokens.clone(),
            cost: g.cost,
        }
    }
}

/// Signed per-field token delta.
#[derive(Clone, Debug, Serialize)]
pub struct TokenBreakdownDelta {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

/// Optional per-field percentage change (`None` when the previous value is zero).
#[derive(Clone, Debug, Serialize)]
pub struct TokenBreakdownPct {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// The full delta between two snapshots for a Group key.
#[derive(Clone, Debug, Serialize)]
pub struct Delta {
    pub sessions: i64,
    pub messages: i64,
    pub tokens: TokenBreakdownDelta,
    pub cost: Option<f64>,
    pub pct: Option<TokenBreakdownPct>,
}

/// One diff row: a Group key with its prev/curr snapshots and the computed delta.
#[derive(Clone, Debug, Serialize)]
pub struct DiffRow {
    pub key: String,
    pub presence: Presence,
    pub prev: Option<Snapshot>,
    pub curr: Option<Snapshot>,
    pub delta: Delta,
}

/// Compute diff rows from two already-aggregated results sharing the same grouping dimension.
///
/// Group keys are matched by exact string equality. Cost delta is only computed when
/// both snapshots carry cost and neither Group was mixed-source. Percentage deltas are
/// `None` for any token field whose previous value is zero.
pub fn compute_diff(prev: &AggregateResult, curr: &AggregateResult) -> Vec<DiffRow> {
    let prev_map: std::collections::BTreeMap<&str, &Group> =
        prev.groups.iter().map(|g| (g.key.as_str(), g)).collect();
    let curr_map: std::collections::BTreeMap<&str, &Group> =
        curr.groups.iter().map(|g| (g.key.as_str(), g)).collect();

    let all_keys: BTreeSet<&str> = prev_map.keys().chain(curr_map.keys()).cloned().collect();

    all_keys
        .into_iter()
        .map(|key| build_row(key, prev_map.get(key).copied(), curr_map.get(key).copied()))
        .collect()
}

fn build_row(key: &str, prev_group: Option<&Group>, curr_group: Option<&Group>) -> DiffRow {
    let prev = prev_group.map(Snapshot::from);
    let curr = curr_group.map(Snapshot::from);

    let presence = match (prev_group.is_some(), curr_group.is_some()) {
        (true, true) => Presence::Both,
        (false, true) => Presence::New,
        (true, false) => Presence::Removed,
        (false, false) => unreachable!(),
    };

    let prev_tokens = prev.as_ref().map(|s| &s.tokens);
    let curr_tokens = curr.as_ref().map(|s| &s.tokens);

    let token_delta = TokenBreakdownDelta {
        input: token_diff(curr_tokens, prev_tokens, |t| t.input),
        output: token_diff(curr_tokens, prev_tokens, |t| t.output),
        cache_read: token_diff(curr_tokens, prev_tokens, |t| t.cache_read),
        cache_write: token_diff(curr_tokens, prev_tokens, |t| t.cache_write),
    };

    let sessions_delta =
        s64(curr.as_ref().map(|s| &s.sessions)) - s64(prev.as_ref().map(|s| &s.sessions));
    let messages_delta =
        s64(curr.as_ref().map(|s| &s.messages)) - s64(prev.as_ref().map(|s| &s.messages));

    let cost_delta = cost_delta(&prev, &curr);
    let pct = pct_delta(prev.as_ref(), &token_delta);

    DiffRow {
        key: key.to_string(),
        presence,
        prev,
        curr,
        delta: Delta {
            sessions: sessions_delta,
            messages: messages_delta,
            tokens: token_delta,
            cost: cost_delta,
            pct,
        },
    }
}

fn s64(opt: Option<&usize>) -> i64 {
    opt.copied().unwrap_or(0) as i64
}

fn token_diff(
    curr: Option<&TokenBreakdown>,
    prev: Option<&TokenBreakdown>,
    field: impl Fn(&TokenBreakdown) -> u64,
) -> i64 {
    let c = curr.map(|t| field(t) as i64).unwrap_or(0);
    let p = prev.map(|t| field(t) as i64).unwrap_or(0);
    c - p
}

/// Cost delta respects ADR-0001: only when both windows carry cost.
fn cost_delta(prev: &Option<Snapshot>, curr: &Option<Snapshot>) -> Option<f64> {
    match (
        prev.as_ref().and_then(|s| s.cost),
        curr.as_ref().and_then(|s| s.cost),
    ) {
        (Some(p), Some(c)) => Some(((c - p) * 1_000_000.0).round() / 1_000_000.0),
        _ => None,
    }
}

fn pct_delta(
    prev: Option<&Snapshot>,
    token_delta: &TokenBreakdownDelta,
) -> Option<TokenBreakdownPct> {
    let prev = prev?;
    let t = &prev.tokens;

    Some(TokenBreakdownPct {
        input: pct(t.input as i64, token_delta.input),
        output: pct(t.output as i64, token_delta.output),
        cache_read: pct(t.cache_read as i64, token_delta.cache_read),
        cache_write: pct(t.cache_write as i64, token_delta.cache_write),
    })
}

fn pct(prev: i64, delta: i64) -> Option<f64> {
    if prev == 0 {
        return None;
    }
    Some(((delta as f64) / (prev as f64) * 100.0 * 10.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;

    #[test]
    fn both_windows_present_produces_delta() {
        let prev = fake_agg(vec![("claude", 1, 10)]);
        let curr = fake_agg(vec![("claude", 1, 20)]);
        let rows = compute_diff(&prev, &curr);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.presence, Presence::Both);
        assert_eq!(row.delta.tokens.input, 10);
        assert!(row.delta.pct.is_some());
        assert_eq!(row.delta.pct.as_ref().unwrap().input.unwrap(), 100.0);
    }

    #[test]
    fn new_key_has_no_prev() {
        let prev = AggregateResult {
            grand_totals: TokenBreakdown::default(),
            grand_messages: 0,
            grand_sessions: 0,
            groups: vec![],
        };
        let curr = fake_agg(vec![("claude", 1, 10)]);
        let rows = compute_diff(&prev, &curr);
        assert_eq!(rows[0].presence, Presence::New);
        assert!(rows[0].prev.is_none());
    }

    #[test]
    fn removed_key_has_no_curr() {
        let prev = fake_agg(vec![("claude", 1, 10)]);
        let curr = AggregateResult {
            grand_totals: TokenBreakdown::default(),
            grand_messages: 0,
            grand_sessions: 0,
            groups: vec![],
        };
        let rows = compute_diff(&prev, &curr);
        assert_eq!(rows[0].presence, Presence::Removed);
        assert!(rows[0].curr.is_none());
    }

    #[test]
    fn cost_delta_none_when_one_side_missing() {
        // Claude has no cost; delta.cost must be None.
        let prev = fake_agg(vec![("claude", 1, 10)]);
        let curr = fake_agg(vec![("claude", 1, 20)]);
        let rows = compute_diff(&prev, &curr);
        assert!(rows[0].delta.cost.is_none());
    }

    #[test]
    fn pct_none_when_prev_zero() {
        let prev = fake_agg(vec![("claude", 1, 0)]);
        let curr = fake_agg(vec![("claude", 1, 50)]);
        let rows = compute_diff(&prev, &curr);
        assert_eq!(rows[0].delta.pct.as_ref().unwrap().input, None);
    }

    #[test]
    fn mixed_keys_union_both_windows() {
        let prev = fake_agg(vec![("a", 1, 10), ("b", 1, 10)]);
        let curr = fake_agg(vec![("a", 1, 20), ("c", 1, 20)]);
        let rows = compute_diff(&prev, &curr);
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert!(keys.contains(&"a"));
        assert!(keys.contains(&"b"));
        assert!(keys.contains(&"c"));
        assert_eq!(
            rows.iter().find(|r| r.key == "a").unwrap().presence,
            Presence::Both
        );
        assert_eq!(
            rows.iter().find(|r| r.key == "b").unwrap().presence,
            Presence::Removed
        );
        assert_eq!(
            rows.iter().find(|r| r.key == "c").unwrap().presence,
            Presence::New
        );
    }

    fn fake_agg(groups: Vec<(&str, usize, u64)>) -> AggregateResult {
        let agg_groups: Vec<Group> = groups
            .into_iter()
            .map(|(k, sessions, input)| Group {
                key: k.to_string(),
                source: k.to_string(),
                sessions,
                messages: sessions * 2,
                tokens: TokenBreakdown {
                    input,
                    output: input,
                    cache_read: 0,
                    cache_write: 0,
                },
                cost: None,
            })
            .collect();
        AggregateResult {
            grand_totals: TokenBreakdown::default(),
            grand_messages: 0,
            grand_sessions: 0,
            groups: agg_groups,
        }
    }

    use chrono::TimeZone;

    fn local(y: i32, mo: u32, d: u32, h: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Local
            .with_ymd_and_hms(y, mo, d, h, 0, 0)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn sliding_pair_matches_the_documented_windows() {
        let now = local(2026, 9, 13, 14);
        let (ps, pe, cs, ce) = window_pair(
            DiffMode::Sliding {
                last: chrono::Duration::days(7),
                prev: chrono::Duration::days(7),
            },
            now,
        )
        .unwrap();
        assert_eq!(ce, now);
        assert_eq!(cs, now - chrono::Duration::days(7));
        assert_eq!(pe, cs);
        assert_eq!(ps, now - chrono::Duration::days(14));
    }

    #[test]
    fn calendar_pair_is_adjacent_and_local_midnight_anchored() {
        let now = local(2026, 9, 13, 14);
        let (ps, pe, cs, ce) = window_pair(
            DiffMode::Calendar {
                last_days: 1,
                prev_days: 1,
            },
            now,
        )
        .unwrap();
        assert_eq!(ce, now);
        // Current bucket starts at today's local midnight.
        assert_eq!(cs, local(2026, 9, 13, 0));
        // Adjacent: the previous window ends exactly where the current begins.
        assert_eq!(pe, cs);
        assert_eq!(ps, local(2026, 9, 12, 0));
    }

    #[test]
    fn calendar_pair_supports_unequal_buckets() {
        let now = local(2026, 9, 13, 14);
        let (ps, pe, cs, ce) = window_pair(
            DiffMode::Calendar {
                last_days: 1,
                prev_days: 7,
            },
            now,
        )
        .unwrap();
        assert_eq!(cs, local(2026, 9, 13, 0));
        assert_eq!(ce, now);
        assert_eq!(pe, cs);
        // Seven local days before today's midnight is 09-06.
        assert_eq!(ps, local(2026, 9, 6, 0));
    }

    #[test]
    fn calendar_pair_week_bucket_trails_six_days() {
        let now = local(2026, 9, 13, 14);
        let (ps, _pe, cs, _ce) = window_pair(
            DiffMode::Calendar {
                last_days: 7,
                prev_days: 7,
            },
            now,
        )
        .unwrap();
        assert_eq!(cs, local(2026, 9, 7, 0));
        assert_eq!(ps, local(2026, 8, 31, 0));
    }
}

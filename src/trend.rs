//! Time-bucketed usage: split a window into aligned, whole time buckets and
//! report the filtered records that fall in each.
//!
//! Every other read command shows a **total** over a window. `trend` answers
//! *when* the usage happened, which a total cannot: a flat 300K/day and one
//! 2M-token afternoon sum to the same week.
//!
//! Two decisions shape this module:
//!
//! 1. **Buckets are aligned and whole, never sliced.** The sequence is built
//!    backwards from `local_midnight(now)` and stepped in *local* days via the
//!    shared [`calendar_bucket_before`], so every bound lands on local midnight
//!    and every bucket is the same width even across a DST transition. Slicing
//!    `[now - last, now]` into equal parts would leave a ragged bucket at each
//!    end and make the rows incomparable.
//! 2. **Empty buckets are rows, not omissions.** "I did nothing that day" and
//!    "there is no data for that day" must be distinguishable, and only an
//!    explicit zero row can say the former. The bucket list is dense by
//!    construction, so a consumer can index it by position.
//!
//! Everything here is a pure function of `now` and the records it is handed:
//! no clock is read, so every boundary is testable with explicit instants.
use crate::aggregator::AggregateResult;
use crate::domain::group::GroupBy;
use crate::domain::record::{Record, TokenBreakdown};
use crate::domain::window::{calendar_bucket_start, local_midnight};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// One half-open `[since, until)` time bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Bucket {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

/// A bucket and the totals of the records that fell in it.
#[derive(Clone, Debug, Serialize)]
pub struct BucketRow {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub sessions: usize,
    pub messages: usize,
    pub tokens: TokenBreakdown,
    /// Source-scoped, exactly as elsewhere: `Some` only when every contributing
    /// record reports a cost *and* they share one source (ADR 0001).
    pub cost: Option<f64>,
}

/// The aligned, whole-bucket sequence of `last_days` ending with the bucket
/// `now` falls in.
///
/// The series is built backwards from `local_midnight(now)` in local-day steps,
/// so every bucket is exactly `bucket_days` local days wide and no partial
/// leading bucket is ever produced. The **last** bucket is the one still open:
/// it ends at `now`, not at a nominal midnight, because "today so far" is
/// genuinely partial.
///
/// The count is `ceil(last_days / bucket_days)`, at least `1`, so a bucket
/// wider than the window yields a single bucket covering the whole span rather
/// than an error.
///
/// Returns `None` only when a local midnight cannot be anchored (an impossible
/// instant, e.g. a DST gap with no valid midnight) — the same contract
/// [`window_bounds`](crate::domain::window::window_bounds) carries.
pub fn bucket_bounds(now: DateTime<Utc>, last_days: u32, bucket_days: u32) -> Option<Vec<Bucket>> {
    let bucket_days = bucket_days.max(1);
    // Anchoring can fail only on an impossible local midnight (a DST gap), the
    // same condition every calendar window shares.
    local_midnight(now)?;
    // Round the span up to a whole number of buckets, so the series always
    // covers `last_days` local days (today inclusive) and every bucket is full.
    //
    // `count` buckets of `bucket_days` each span `count * bucket_days` days,
    // ending on today's midnight. `calendar_bucket_start(now, n)` is exactly the
    // start of the `n`-day bucket ending today, so the oldest bucket is
    // `calendar_bucket_start(now, span)` and each later one steps in by
    // `bucket_days`. (Not `calendar_bucket_before`: that returns the bucket
    // *before* its argument and clamps a zero step to one day, which would shift
    // the series.)
    let count = last_days.div_ceil(bucket_days).max(1);
    let span = count * bucket_days;

    let mut buckets = Vec::with_capacity(count as usize);
    for i in 0..count {
        let since =
            calendar_bucket_start(now, span - i * bucket_days).expect("span >= 1 by construction");
        let until = if i + 1 == count {
            now
        } else {
            calendar_bucket_start(now, span - (i + 1) * bucket_days)
                .expect("span >= 1 by construction")
        };
        buckets.push(Bucket { since, until });
    }
    Some(buckets)
}

/// Place each record in the bucket whose half-open range contains its
/// `started_at`, and total each bucket.
///
/// A record falls in **at most one** bucket by construction, so totals never
/// double-count. A record whose `started_at` lies outside every bucket is
/// dropped rather than force-fit into an edge bucket: this cannot arise from a
/// filter built by `bucket_bounds`' own window, but silently attributing it
/// would corrupt a boundary the caller cannot see.
///
/// Per-bucket totals come from [`AggregateResult::from_filtered_refs`] — the
/// same function `usage` calls — so sessions, messages, tokens, and the
/// source-scoped cost rule cannot drift between the two commands. Rows are
/// returned in the order `buckets` was given (oldest-first).
pub fn bucketize(records: &[&Record], buckets: &[Bucket]) -> Vec<BucketRow> {
    buckets
        .iter()
        .map(|bucket| {
            let in_bucket: Vec<&Record> = records
                .iter()
                .filter(|r| r.started_at >= bucket.since && r.started_at < bucket.until)
                .copied()
                .collect();
            let agg = AggregateResult::from_filtered_refs(&in_bucket, GroupBy::Source);
            // The grand totals are the bucket's totals; taking them from the
            // shared aggregator (rather than summing here) is what keeps the
            // token shape identical to every other command.
            //
            // Cost is a separate question, and the one ADR 0001 governs. The
            // bucket's cost is known only when the whole bucket draws on a
            // single source and every record reports one — otherwise it is `None`
            // (`—`), never a sum across sources. Aggregating by `Source` above
            // makes each group already carry that per-source answer, so the rule
            // here reduces to: exactly one group, and it has a cost.
            //
            // This is stricter than summing the groups' costs: two cost-bearing
            // sources in one bucket would add up to a number with no common
            // currency basis, which is precisely what ADR 0001 forbids.
            let cost = match agg.groups.as_slice() {
                [only] => only.cost,
                _ => None,
            };
            BucketRow {
                since: bucket.since,
                until: bucket.until,
                sessions: agg.grand_sessions,
                messages: agg.grand_messages,
                tokens: agg.grand_totals,
                cost,
            }
        })
        .collect()
}

/// Human-readable bucket label for the terminal table.
///
/// A one-day bucket prints its local date (`2026-08-23`); a wider bucket prints
/// its span (`2026-08-23 → 08-30`). Labelling a weekly bucket with only its
/// start date would read as if the row were that single day's usage, which is
/// exactly the misreading a trend exists to prevent.
///
/// `bucket_days` is the series' nominal width, passed in rather than inferred
/// from the bounds: the final bucket is open and ends at `now`, so its measured
/// width is a partial day even when its nominal width is a week.
///
/// Rendered in `Local`, not UTC: the boundaries were chosen in local days, so
/// labelling them in UTC would print a date that disagrees with the bucket it
/// names near a midnight boundary.
pub fn bucket_label(bucket: &Bucket, bucket_days: u32) -> String {
    let since = bucket.since.with_timezone(&chrono::Local);
    if bucket_days <= 1 {
        return since.format("%Y-%m-%d").to_string();
    }
    let until = bucket.until.with_timezone(&chrono::Local);
    format!("{} → {}", since.format("%Y-%m-%d"), until.format("%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Local, TimeZone};

    /// A local-wall-clock instant converted to UTC. Expectations are derived
    /// from `Local` rather than a hardcoded offset, so the tests hold in any
    /// timezone — the helper spec 0017 established.
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .or_else(|| Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest())
            .unwrap()
            .with_timezone(&Utc)
    }

    fn rec(source: &str, started_at: DateTime<Utc>, cost: Option<f64>) -> Record {
        Record {
            session_id: format!("ses_{}_{}", source, started_at),
            source: source.to_string(),
            project: "/p".to_string(),
            model: "m".to_string(),
            agent: None,
            started_at,
            ended_at: None,
            tokens: TokenBreakdown {
                input: 10,
                output: 1,
                cache_read: 0,
                cache_write: 0,
            },
            message_count: 1,
            cost,
        }
    }

    #[test]
    fn bucket_count_rounds_up_for_a_non_dividing_window() {
        // 30 days in 7-day buckets needs 5 rows to cover the span (4*7 = 28 < 30).
        let now = local(2026, 9, 30, 14, 0);
        let buckets = bucket_bounds(now, 30, 7).unwrap();
        assert_eq!(buckets.len(), 5);
    }

    #[test]
    fn bucket_count_is_exact_when_the_window_divides() {
        let now = local(2026, 9, 30, 14, 0);
        let buckets = bucket_bounds(now, 30, 1).unwrap();
        assert_eq!(buckets.len(), 30);
    }

    #[test]
    fn every_bucket_bound_lands_on_local_midnight() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 14, 1).unwrap();
        for b in &buckets {
            // `since` is always a local midnight.
            assert_eq!(
                local_midnight(b.since).unwrap(),
                b.since,
                "bucket since {} is not local midnight",
                b.since
            );
        }
        // The closing bound of every non-final bucket is local midnight too.
        for b in &buckets[..buckets.len() - 1] {
            assert_eq!(local_midnight(b.until).unwrap(), b.until);
        }
    }

    #[test]
    fn buckets_are_contiguous_with_no_gaps_or_overlaps() {
        let now = local(2026, 9, 30, 9, 15);
        let buckets = bucket_bounds(now, 10, 2).unwrap();
        for w in buckets.windows(2) {
            assert_eq!(w[0].until, w[1].since);
        }
    }

    #[test]
    fn the_last_bucket_is_the_open_one_ending_at_now() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 3, 1).unwrap();
        let last = buckets.last().unwrap();
        assert_eq!(last.until, now);
        // ...and it starts on today's local midnight.
        assert_eq!(last.since, local_midnight(now).unwrap());
    }

    #[test]
    fn buckets_are_oldest_first() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 5, 1).unwrap();
        for w in buckets.windows(2) {
            assert!(w[0].since < w[1].since);
        }
    }

    #[test]
    fn the_series_ends_today_and_covers_the_requested_span() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 7, 1).unwrap();
        // 7 rows: today and the six days before it.
        assert_eq!(buckets.len(), 7);
        assert_eq!(buckets[0].since, local(2026, 9, 24, 0, 0));
        assert_eq!(buckets[6].since, local(2026, 9, 30, 0, 0));
    }

    #[test]
    fn a_bucket_wider_than_the_window_yields_one_bucket() {
        // `--bucket 1mo` (30 days) over `--last 7d`: one bucket, not an error —
        // and it must span the whole window, not collapse onto today.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 7, 30).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].until, now);
        // A 30-day bucket ending today starts on 09-01.
        assert_eq!(buckets[0].since, local(2026, 9, 1, 0, 0));
    }

    #[test]
    fn a_multi_day_bucket_covers_the_whole_window() {
        // The regression guard for the bug this series was rewritten to fix: a
        // `--last 7d --bucket 1w` trend was one bucket anchored on *today*, so it
        // showed only today's usage while claiming to be a week.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 7, 7).unwrap();
        assert_eq!(buckets.len(), 1);
        // The single bucket opens seven local days before today, not today.
        assert_eq!(buckets[0].since, local(2026, 9, 24, 0, 0));
        assert_eq!(buckets[0].until, now);
        assert_eq!(
            buckets[0].since,
            local_midnight(now).unwrap() - Duration::days(6)
        );
    }

    #[test]
    fn the_span_rounds_up_to_whole_buckets() {
        // 30 days of 7-day buckets needs 5 buckets (35 days of coverage): the
        // series may over-cover, but it must never under-cover the window.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 30, 7).unwrap();
        assert_eq!(buckets.len(), 5);
        // The oldest bucket opens 34 local days before today (5 * 7 - 1).
        assert_eq!(
            buckets[0].since,
            local_midnight(now).unwrap() - Duration::days(34)
        );
    }

    #[test]
    fn a_zero_day_window_still_yields_one_bucket() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 0, 1).unwrap();
        assert_eq!(buckets.len(), 1);
    }

    #[test]
    fn a_zero_day_bucket_is_treated_as_one_day() {
        let now = local(2026, 9, 30, 14, 32);
        let zero = bucket_bounds(now, 3, 0).unwrap();
        let one = bucket_bounds(now, 3, 1).unwrap();
        assert_eq!(zero, one);
    }

    #[test]
    fn a_record_on_the_lower_bound_is_included_and_one_on_the_upper_is_not() {
        // Half-open `[since, until)`: the boundary record belongs to the bucket
        // that starts at it, never to both.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 2, 1).unwrap();
        let second_start = buckets[1].since;
        let on_lower = rec("claude", second_start, None);
        // A record exactly on the second bucket's upper bound (`now`) lands
        // outside every bucket, because the final bucket is half-open too.
        let on_upper = rec("claude", now, None);
        let rows = bucketize(&[&on_lower, &on_upper], &buckets);
        assert_eq!(rows[0].sessions, 0);
        assert_eq!(
            rows[1].sessions, 1,
            "lower-bound record belongs to its bucket"
        );
        let total: usize = rows.iter().map(|r| r.sessions).sum();
        assert_eq!(
            total, 1,
            "the upper-bound record is excluded, not double-counted"
        );
    }

    #[test]
    fn a_record_outside_every_bucket_is_dropped() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 2, 1).unwrap();
        let ancient = rec("claude", local(2020, 1, 1, 0, 0), None);
        let future = rec("claude", local(2030, 1, 1, 0, 0), None);
        let rows = bucketize(&[&ancient, &future], &buckets);
        let total: usize = rows.iter().map(|r| r.sessions).sum();
        assert_eq!(total, 0);
    }

    #[test]
    fn no_record_is_counted_in_two_buckets() {
        // Two records, each in a distinct bucket: the totals partition.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 3, 1).unwrap();
        let a = rec("claude", buckets[0].since, None);
        let b = rec("claude", buckets[2].since, None);
        let rows = bucketize(&[&a, &b], &buckets);
        assert_eq!(rows.iter().map(|r| r.sessions).sum::<usize>(), 2);
        assert_eq!(rows[1].sessions, 0, "the middle bucket stays empty");
    }

    #[test]
    fn empty_buckets_are_emitted_as_zero_rows() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 4, 1).unwrap();
        let rows = bucketize(&[], &buckets);
        assert_eq!(rows.len(), 4);
        for r in &rows {
            assert_eq!(r.sessions, 0);
            assert_eq!(r.messages, 0);
            assert_eq!(r.tokens, TokenBreakdown::default());
            assert!(r.cost.is_none());
        }
    }

    #[test]
    fn a_single_source_cost_reporting_bucket_sums_its_cost() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 1, 1).unwrap();
        let a = rec("opencode", buckets[0].since, Some(0.25));
        let b = rec("opencode", buckets[0].since, Some(0.75));
        let rows = bucketize(&[&a, &b], &buckets);
        assert_eq!(rows[0].cost, Some(1.0));
    }

    #[test]
    fn a_bucket_mixing_cost_reporting_and_non_reporting_sources_has_no_cost() {
        // ADR 0001: a cost that spans a source that records none is unknown, not
        // a silent undercount. Claude reports no cost, so the bucket's is None.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 1, 1).unwrap();
        let claude = rec("claude", buckets[0].since, None);
        let opencode = rec("opencode", buckets[0].since, Some(0.40));
        let rows = bucketize(&[&claude, &opencode], &buckets);
        assert_eq!(rows[0].cost, None);
        // The tokens still add up — only the cost is withheld.
        assert_eq!(rows[0].sessions, 2);
        assert_eq!(rows[0].tokens.input, 20);
    }

    #[test]
    fn a_bucket_mixing_two_cost_reporting_sources_has_no_cost() {
        // The strictest case of ADR 0001. Both sources report a cost, so a naive
        // "sum whatever has a cost" would yield 0.65 — a number with no common
        // currency basis. The rule is that cost is scoped to a **single source**,
        // so a two-source bucket is unpriced regardless.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 1, 1).unwrap();
        let kilo = rec("kilo", buckets[0].since, Some(0.25));
        let opencode = rec("opencode", buckets[0].since, Some(0.40));
        let rows = bucketize(&[&kilo, &opencode], &buckets);
        assert_eq!(
            rows[0].cost, None,
            "cost must never be summed across two sources"
        );
        // Tokens, by contrast, have a common unit and do add up.
        assert_eq!(rows[0].sessions, 2);
        assert_eq!(rows[0].tokens.input, 20);
    }

    #[test]
    fn bucket_label_uses_the_local_calendar_date() {
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 1, 1).unwrap();
        assert_eq!(bucket_label(&buckets[0], 1), "2026-09-30");
    }

    #[test]
    fn bucket_label_shows_a_span_for_multi_day_buckets() {
        // A weekly bucket must not read as a single date: the row covers seven
        // days, and the label says so. The final (open) bucket ends at `now`,
        // so its span ends on today's date.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 7, 7).unwrap();
        assert_eq!(bucket_label(&buckets[0], 7), "2026-09-24 → 09-30");

        // With two weekly buckets, the closed one ends on the next midnight.
        let buckets = bucket_bounds(now, 14, 7).unwrap();
        assert_eq!(buckets.len(), 2);
        assert_eq!(bucket_label(&buckets[0], 7), "2026-09-17 → 09-24");
        assert_eq!(bucket_label(&buckets[1], 7), "2026-09-24 → 09-30");
    }

    #[test]
    fn a_long_series_does_not_drift() {
        // A longer series must not drift: the oldest bucket starts on the local
        // midnight 89 days before today's, and every step is exactly one local
        // day. Expectations are built from local calendar dates rather than
        // `now - Duration::days(n)`, which a DST transition would falsify.
        let now = local(2026, 9, 30, 14, 32);
        let buckets = bucket_bounds(now, 90, 1).unwrap();
        assert_eq!(buckets.len(), 90);
        assert_eq!(buckets[0].since, local(2026, 7, 3, 0, 0));
        assert_eq!(buckets[89].since, local_midnight(now).unwrap());
    }
}

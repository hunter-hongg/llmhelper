//! Calendar-window vocabulary shared by `budget`, `diff`, `usage`, and
//! `report`.
//!
//! All four commands need to answer "what is a day?" and must agree: a `1d`
//! budget resets, a `1d` diff bucket starts, and a `usage`/`report` calendar
//! window opens at the **same** instant. Rather than define local-midnight math
//! four times, the bucket helpers and the single-window derivation live here
//! and every caller delegates.
//!
//! Everything is a pure function of `now`: no clock is read, so every boundary
//! is testable with explicit timestamps.

use chrono::{DateTime, Duration, Local, TimeZone, Utc};

/// How a single window is anchored in time — the shared vocabulary behind
/// `usage`/`report`'s `--calendar` and (composed pairwise) `diff`'s windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMode {
    /// A rolling window relative to `now`: `[now - last, now]`.
    Rolling { last: Duration },
    /// A trailing local-day bucket: `[calendar_bucket_start(now, days), now]`.
    /// `days == 1` is "today so far" (local midnight → now).
    Calendar { days: u32 },
}

/// The half-open `(since, until)` bounds for `mode` at `now`. Pure: reads no
/// clock. Returns `None` only when a calendar bucket cannot be anchored (an
/// impossible local midnight, e.g. a DST gap with no valid instant).
pub fn window_bounds(
    mode: WindowMode,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    match mode {
        WindowMode::Rolling { last } => Some((now - last, now)),
        WindowMode::Calendar { days } => Some((calendar_bucket_start(now, days)?, now)),
    }
}

/// The calendar keywords `budget`, `diff`, `usage`, and `report` recognize,
/// mapped to a bucket length in **local** days. `1d` is "today", `1w` is the
/// trailing 7 local days inclusive of today, `1mo` is the trailing 30 local
/// days. These are trailing windows ending at the current local day — not ISO
/// weeks or calendar months.
pub fn calendar_days(s: &str) -> Option<u32> {
    match s {
        "1d" => Some(1),
        "1w" => Some(7),
        "1mo" => Some(30),
        _ => None,
    }
}

/// The start of the local calendar day containing `now`, expressed in UTC.
///
/// Anchoring to *local* midnight is deliberate: "today" means the user's day,
/// not UTC's. On a DST spring-forward gap the nominal midnight may not exist;
/// we take the earliest valid instant instead of failing.
pub fn local_midnight(now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let local_now = now.with_timezone(&Local);
    let start_local = local_now.date_naive().and_hms_opt(0, 0, 0)?;
    let start_local = Local
        .from_local_datetime(&start_local)
        .single()
        .or_else(|| Local.from_local_datetime(&start_local).earliest())?;
    Some(start_local.with_timezone(&Utc))
}

/// The start of a trailing calendar bucket of `days` local days ending on
/// `now`'s local day: `local_midnight(now) - (days - 1)`. `days == 1` is the
/// start of today; `days == 7` is the start of "today and the six days before
/// it". `days == 0` is treated as `days == 1` (there is no empty day).
pub fn calendar_bucket_start(now: DateTime<Utc>, days: u32) -> Option<DateTime<Utc>> {
    let days = days.max(1);
    Some(local_midnight(now)? - Duration::days((days as i64) - 1))
}

/// The start of the `days`-long calendar bucket that ends exactly where the
/// local-day bucket containing `anchor` begins: `local_midnight(anchor) -
/// days`. Stepping back in **local** day units (not raw UTC hours) keeps the
/// boundary on local midnight even across a DST transition. `days == 1` is the
/// single local day immediately before `anchor`'s day.
pub fn calendar_bucket_before(anchor: DateTime<Utc>, days: u32) -> Option<DateTime<Utc>> {
    let days = days.max(1);
    let local_anchor = local_midnight(anchor)?.with_timezone(&Local);
    let start_local = local_anchor.date_naive().and_hms_opt(0, 0, 0)? - Duration::days(days as i64);
    let start_local = Local
        .from_local_datetime(&start_local)
        .single()
        .or_else(|| Local.from_local_datetime(&start_local).earliest())?;
    Some(start_local.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// A local-wall-clock instant on the machine's own timezone, converted to
    /// UTC. Tests derive their expectations from `Local` rather than a
    /// hardcoded offset, so they hold on any machine.
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .or_else(|| Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest())
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn calendar_days_maps_the_three_keywords() {
        assert_eq!(calendar_days("1d"), Some(1));
        assert_eq!(calendar_days("1w"), Some(7));
        assert_eq!(calendar_days("1mo"), Some(30));
        assert_eq!(calendar_days("4h"), None);
        assert_eq!(calendar_days("2d"), None);
        assert_eq!(calendar_days(""), None);
    }

    #[test]
    fn local_midnight_is_the_local_day_start() {
        // 14:32 local on 2026-09-13 → midnight is 00:00 local the same day.
        let now = local(2026, 9, 13, 14, 32);
        assert_eq!(local_midnight(now).unwrap(), local(2026, 9, 13, 0, 0));
    }

    #[test]
    fn local_midnight_before_dawn_is_the_previous_instance() {
        // 02:00 local is still the same calendar day's bucket.
        let now = local(2026, 9, 13, 2, 0);
        assert_eq!(local_midnight(now).unwrap(), local(2026, 9, 13, 0, 0));
    }

    #[test]
    fn bucket_start_for_one_day_is_today() {
        let now = local(2026, 9, 13, 14, 32);
        assert_eq!(calendar_bucket_start(now, 1), local_midnight(now));
    }

    #[test]
    fn bucket_start_for_seven_days_trails_six_days() {
        let now = local(2026, 9, 13, 14, 32);
        let start = calendar_bucket_start(now, 7).unwrap();
        // Today is 09-13; six days earlier is 09-07, at local midnight.
        assert_eq!(start, local(2026, 9, 7, 0, 0));
    }

    #[test]
    fn bucket_start_for_thirty_days() {
        let now = local(2026, 9, 30, 9, 0);
        let start = calendar_bucket_start(now, 30).unwrap();
        // 30 days inclusive of 09-30 → start is 09-01.
        assert_eq!(start, local(2026, 9, 1, 0, 0));
    }

    #[test]
    fn bucket_start_clamps_zero_days_to_one() {
        let now = local(2026, 9, 13, 14, 32);
        assert_eq!(calendar_bucket_start(now, 0), calendar_bucket_start(now, 1));
    }

    #[test]
    fn rolling_bounds_start_at_now_minus_last() {
        let now = local(2026, 9, 13, 14, 32);
        let (since, until) = window_bounds(
            WindowMode::Rolling {
                last: Duration::days(7),
            },
            now,
        )
        .unwrap();
        assert_eq!(since, now - Duration::days(7));
        assert_eq!(until, now);
    }

    #[test]
    fn calendar_bounds_open_at_the_bucket_start() {
        let now = local(2026, 9, 13, 14, 32);
        // `1d` is "today so far": local midnight → now, *not* a rolling 24h.
        let (since, until) = window_bounds(WindowMode::Calendar { days: 1 }, now).unwrap();
        assert_eq!(since, local(2026, 9, 13, 0, 0));
        assert_eq!(until, now);
        assert_eq!(since, local_midnight(now).unwrap());
    }

    #[test]
    fn calendar_bounds_for_a_week_trail_six_days() {
        let now = local(2026, 9, 13, 14, 32);
        let (since, until) = window_bounds(WindowMode::Calendar { days: 7 }, now).unwrap();
        assert_eq!(since, local(2026, 9, 7, 0, 0));
        assert_eq!(until, now);
    }

    #[test]
    fn calendar_bounds_differ_from_rolling_at_a_day_boundary() {
        // 00:30 local: a `1d` calendar window is just 30 minutes wide, while a
        // `1d` rolling window reaches back into yesterday. The two must not be
        // interchangeable — this is the whole point of `--calendar`.
        let now = local(2026, 9, 13, 0, 30);
        let (cal_since, _) = window_bounds(WindowMode::Calendar { days: 1 }, now).unwrap();
        let (roll_since, _) = window_bounds(
            WindowMode::Rolling {
                last: Duration::days(1),
            },
            now,
        )
        .unwrap();
        assert_eq!(cal_since, local(2026, 9, 13, 0, 0));
        assert_eq!(roll_since, local(2026, 9, 12, 0, 30));
        assert!(cal_since > roll_since);
    }

    #[test]
    fn bucket_before_is_a_full_bucket_ending_at_the_anchor() {
        // Anchor at local midnight 09-13; the day immediately before is 09-12.
        let anchor = local(2026, 9, 13, 0, 0);
        assert_eq!(
            calendar_bucket_before(anchor, 1).unwrap(),
            local(2026, 9, 12, 0, 0)
        );
        // A 7-day bucket before 09-13 starts on 09-06.
        assert_eq!(
            calendar_bucket_before(anchor, 7).unwrap(),
            local(2026, 9, 6, 0, 0)
        );
    }

    #[test]
    fn bucket_before_uses_the_anchors_local_day() {
        // An anchor mid-day still resolves to local midnight before stepping back.
        let anchor = local(2026, 9, 13, 14, 32);
        assert_eq!(
            calendar_bucket_before(anchor, 1).unwrap(),
            local(2026, 9, 12, 0, 0)
        );
    }

    #[test]
    fn bucket_before_never_returns_the_anchor_itself() {
        // The contract is the *previous* bucket, so even a 0-day request steps
        // back a full day. `diff` and the TUI's bucket cycling chain this call,
        // and assuming `before(anchor, 0) == anchor` would collapse two adjacent
        // buckets onto one instant.
        let anchor = local(2026, 9, 13, 0, 0);
        assert_eq!(
            calendar_bucket_before(anchor, 0).unwrap(),
            local(2026, 9, 12, 0, 0)
        );
        assert_eq!(
            calendar_bucket_before(anchor, 0).unwrap(),
            calendar_bucket_before(anchor, 1).unwrap()
        );
    }

    #[test]
    fn chained_bucket_before_walks_back_one_whole_bucket_at_a_time() {
        // Chaining yields consecutive, non-overlapping buckets with no duplicate
        // and no gap — the property callers that walk backwards rely on.
        let anchor = local(2026, 9, 13, 0, 0);
        let mut cursor = anchor;
        let mut days = vec![cursor];
        for _ in 0..4 {
            cursor = calendar_bucket_before(cursor, 1).unwrap();
            days.push(cursor);
        }
        assert_eq!(
            days,
            vec![
                local(2026, 9, 13, 0, 0),
                local(2026, 9, 12, 0, 0),
                local(2026, 9, 11, 0, 0),
                local(2026, 9, 10, 0, 0),
                local(2026, 9, 9, 0, 0),
            ]
        );
    }
}

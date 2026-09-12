use chrono::{DateTime, Utc};
use std::time::Duration;

use crate::domain::message::Message;
use crate::domain::record::Record;
use crate::domain::window::{window_bounds, WindowMode};

/// Pre-aggregation filters applied to a record set.
/// All predicates AND-combine; an empty filter passes everything through.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub since: Option<DateTime<Utc>>,
    pub last: Option<Duration>,
    /// Upper bound for the time window (inclusive). Only used by `diff` to
    /// express the previous window's end.
    pub until: Option<DateTime<Utc>>,
    pub project: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    /// When true, a message without a recorded timestamp passes any active
    /// time predicate instead of failing closed. Records always carry a
    /// timestamp, so this only affects message search. Default `false`.
    pub fail_open: bool,
}

impl Filter {
    /// Build a filter constrained to a half-open interval `[since, until]`.
    /// Used by `diff` to slice two distinct time windows.
    pub fn within(since: DateTime<Utc>, until: DateTime<Utc>) -> Self {
        Self {
            since: Some(since),
            until: Some(until),
            ..Self::default()
        }
    }
}

/// Apply `mode`'s window, resolved at `now`, on top of a command's
/// non-temporal predicates. Returns `None` only when a calendar bucket cannot
/// be anchored (an impossible local midnight, e.g. a DST gap). `mode == None`
/// means "no time bound" and yields the base filter unchanged.
///
/// `now` is a parameter rather than a clock read so callers can rebuild the
/// window on every load: a calendar bucket then stays pinned to local midnight
/// while `now` advances, instead of drifting with the current clock time.
pub fn window_filter(
    base: &Filter,
    mode: Option<WindowMode>,
    now: DateTime<Utc>,
) -> Option<Filter> {
    let Some(mode) = mode else {
        return Some(base.clone());
    };
    let (since, until) = window_bounds(mode, now)?;
    Some(Filter {
        since: Some(since),
        until: Some(until),
        ..base.clone()
    })
}

impl Filter {
    pub fn none() -> Self {
        Self::default()
    }

    /// Check whether a single record satisfies this filter.
    pub fn matches(&self, r: &Record) -> bool {
        self.matches_at(Some(r.started_at), &r.source, &r.project, &r.model)
    }

    /// Check whether a single message satisfies this filter.
    ///
    /// Records always have a timestamp; messages may not. A message without a
    /// timestamp fails any time predicate rather than passing through, so a
    /// scoped search cannot silently include unscoped content.
    pub fn matches_message(&self, m: &Message) -> bool {
        self.matches_at(
            m.timestamp,
            &m.source,
            &m.project,
            m.model.as_deref().unwrap_or(""),
        )
    }

    /// Shared predicate body for records and messages. Both must be filtered by
    /// exactly the same rules, so neither method may drift from this one.
    fn matches_at(
        &self,
        at: Option<DateTime<Utc>>,
        source: &str,
        project: &str,
        model: &str,
    ) -> bool {
        // A missing timestamp normally fails any active time predicate
        // (fail-closed). With `fail_open`, the timestamp is treated as
        // unknown rather than out-of-window, so the entry passes through.
        let timestamp_known = at.is_some();
        if let Some(since) = self.since {
            if at.is_none_or(|t| t < since) && !(self.fail_open && !timestamp_known) {
                return false;
            }
        }
        if let Some(window) = self.last {
            let cutoff = Utc::now() - window;
            if at.is_none_or(|t| t < cutoff) && !(self.fail_open && !timestamp_known) {
                return false;
            }
        }
        if let Some(until) = self.until {
            if at.is_none_or(|t| t > until) && !(self.fail_open && !timestamp_known) {
                return false;
            }
        }
        if let Some(proj) = &self.project {
            if !project.contains(proj.as_str()) {
                return false;
            }
        }
        if let Some(filter_model) = &self.model {
            if !model.to_lowercase().contains(&filter_model.to_lowercase()) {
                return false;
            }
        }
        if let Some(src) = &self.source {
            if source != *src {
                return false;
            }
        }
        true
    }

    /// Apply all predicates and return a filtered view.
    pub fn apply<'a>(&self, records: &'a [Record]) -> Vec<&'a Record> {
        records.iter().filter(|r| self.matches(r)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;

    fn rec(source: &str, project: &str, model: &str, started_at: DateTime<Utc>) -> Record {
        Record {
            session_id: format!("ses_{}", source),
            source: source.to_string(),
            project: project.to_string(),
            model: model.to_string(),
            agent: None,
            started_at,
            ended_at: None,
            tokens: TokenBreakdown::default(),
            message_count: 1,
            cost: None,
        }
    }

    #[test]
    fn empty_filter_retains_all() {
        let r = vec![
            rec("a", "/p", "m", Utc::now()),
            rec("b", "/q", "n", Utc::now()),
        ];
        assert_eq!(Filter::none().apply(&r).len(), 2);
    }

    #[test]
    fn filter_source() {
        let r = vec![
            rec("claude", "/p", "m", Utc::now()),
            rec("opencode", "/p", "m", Utc::now()),
        ];
        let f = Filter {
            source: Some("claude".to_string()),
            ..Default::default()
        };
        assert_eq!(f.apply(&r).len(), 1);
        assert_eq!(f.apply(&r)[0].source, "claude");
    }

    #[test]
    fn filter_project_substring() {
        let r = vec![
            rec("a", "/home/user/proj-x", "m", Utc::now()),
            rec("a", "/other/y", "m", Utc::now()),
        ];
        let f = Filter {
            project: Some("proj".to_string()),
            ..Default::default()
        };
        assert_eq!(f.apply(&r).len(), 1);
    }

    #[test]
    fn filter_model_case_insensitive() {
        let r = vec![
            rec("a", "/p", "Big-Pickle", Utc::now()),
            rec("a", "/p", "Auto", Utc::now()),
        ];
        let f = Filter {
            model: Some("big".to_string()),
            ..Default::default()
        };
        assert_eq!(f.apply(&r).len(), 1);
        assert_eq!(f.apply(&r)[0].model, "Big-Pickle");
    }

    #[test]
    fn filter_since() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(30);
        let r = vec![rec("a", "/p", "m", old), rec("a", "/p", "m", now)];
        let f = Filter {
            since: Some(now - chrono::Duration::days(7)),
            ..Default::default()
        };
        assert_eq!(f.apply(&r).len(), 1);
    }

    #[test]
    fn filter_last() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(60);
        let r = vec![rec("a", "/p", "m", old), rec("a", "/p", "m", now)];
        let f = Filter {
            last: Some(Duration::from_secs(30 * 24 * 3600)),
            ..Default::default()
        };
        assert_eq!(f.apply(&r).len(), 1);
    }

    fn message(
        source: &str,
        project: &str,
        model: Option<&str>,
        timestamp: Option<DateTime<Utc>>,
    ) -> Message {
        Message {
            source: source.to_string(),
            session_id: "s1".to_string(),
            project: project.to_string(),
            model: model.map(str::to_string),
            role: "assistant".to_string(),
            timestamp,
            text: "body".to_string(),
        }
    }

    #[test]
    fn message_filter_matches_record_filter() {
        // Same shape as `rec`, but as a message: every predicate that admits a
        // record must admit the equivalent message and vice versa.
        let f = Filter {
            since: Some(Utc::now() - chrono::Duration::days(7)),
            project: Some("proj".to_string()),
            model: Some("big".to_string()),
            source: Some("claude".to_string()),
            ..Default::default()
        };
        let now = Utc::now();
        let m = message("claude", "/home/user/proj-x", Some("Big-Pickle"), Some(now));
        let r = rec("claude", "/home/user/proj-x", "Big-Pickle", now);
        assert!(f.matches(&r));
        assert!(f.matches_message(&m));
    }

    #[test]
    fn message_filter_rejects_the_same_shape_records_are_rejected() {
        let f = Filter {
            project: Some("proj".to_string()),
            ..Default::default()
        };
        assert!(!f.matches_message(&message("a", "/other/y", None, None)));
    }

    #[test]
    fn message_filter_drops_untimestamped_messages_on_time_filters() {
        let since_f = Filter {
            since: Some(Utc::now() - chrono::Duration::days(7)),
            ..Default::default()
        };
        assert!(!since_f.matches_message(&message("a", "/p", None, None)));
        assert!(since_f.matches_message(&message("a", "/p", None, Some(Utc::now()))));

        let last_f = Filter {
            last: Some(Duration::from_secs(3600)),
            ..Default::default()
        };
        assert!(!last_f.matches_message(&message("a", "/p", None, None)));

        let until_f = Filter {
            until: Some(Utc::now()),
            ..Default::default()
        };
        assert!(!until_f.matches_message(&message("a", "/p", None, None)));
    }

    #[test]
    fn message_filter_without_time_filters_admits_untimestamped_messages() {
        let f = Filter {
            project: Some("proj".to_string()),
            ..Default::default()
        };
        assert!(f.matches_message(&message("a", "/p/proj", None, None)));
        // A message with no recorded model cannot satisfy a model filter.
        let model_f = Filter {
            model: Some("auto".to_string()),
            ..Default::default()
        };
        assert!(!model_f.matches_message(&message("a", "/p", None, None)));
        assert!(model_f.matches_message(&message("a", "/p", Some("auto"), None)));
    }

    #[test]
    fn message_filter_fail_open_passes_untimestamped_messages_through_time_filters() {
        let since_f = Filter {
            since: Some(Utc::now() - chrono::Duration::days(7)),
            fail_open: true,
            ..Default::default()
        };
        assert!(since_f.matches_message(&message("a", "/p", None, None)));

        let last_f = Filter {
            last: Some(Duration::from_secs(3600)),
            fail_open: true,
            ..Default::default()
        };
        assert!(last_f.matches_message(&message("a", "/p", None, None)));

        let until_f = Filter {
            until: Some(Utc::now()),
            fail_open: true,
            ..Default::default()
        };
        assert!(until_f.matches_message(&message("a", "/p", None, None)));

        // fail_open never relaxes non-time predicates.
        let project_f = Filter {
            project: Some("proj".to_string()),
            since: Some(Utc::now()),
            fail_open: true,
            ..Default::default()
        };
        assert!(!project_f.matches_message(&message("a", "/other", None, None)));
        assert!(project_f.matches_message(&message("a", "/p/proj", None, None)));
    }

    #[test]
    fn message_filter_fail_closed_by_default() {
        let since_f = Filter {
            since: Some(Utc::now() - chrono::Duration::days(7)),
            ..Default::default()
        };
        assert!(!since_f.matches_message(&message("a", "/p", None, None)));
    }

    /// A local-wall-clock instant on the machine's own timezone, converted to
    /// UTC. Expectations derive from `Local`, so they hold on any machine.
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        use chrono::{Local, TimeZone};
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .or_else(|| Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest())
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn window_filter_without_mode_leaves_base_unchanged() {
        let base = Filter {
            project: Some("proj".to_string()),
            ..Default::default()
        };
        let got = window_filter(&base, None, local(2026, 9, 12, 20, 20)).unwrap();
        assert_eq!(got.since, None);
        assert_eq!(got.until, None);
        assert_eq!(got.project.as_deref(), Some("proj"));
    }

    #[test]
    fn window_filter_rolling_spans_now_minus_last() {
        let now = local(2026, 9, 12, 20, 20);
        let last = chrono::Duration::hours(6);
        let got = window_filter(&Filter::none(), Some(WindowMode::Rolling { last }), now).unwrap();
        assert_eq!(got.since, Some(now - last));
        assert_eq!(got.until, Some(now));
    }

    #[test]
    fn window_filter_calendar_opens_at_local_midnight() {
        let now = local(2026, 9, 12, 20, 20);
        let got =
            window_filter(&Filter::none(), Some(WindowMode::Calendar { days: 1 }), now).unwrap();
        assert_eq!(got.since, Some(local(2026, 9, 12, 0, 0)));
        assert_eq!(got.until, Some(now));
    }

    /// The whole point of carrying `WindowMode` instead of a pre-built window:
    /// as the TUI refreshes and `now` advances past local midnight, the bucket
    /// re-anchors to the *new* day rather than sliding with the clock.
    #[test]
    fn window_filter_rebuilds_a_calendar_bucket_per_load() {
        let before_midnight = local(2026, 9, 12, 23, 59);
        let after_midnight = local(2026, 9, 13, 0, 1);
        let mode = Some(WindowMode::Calendar { days: 1 });

        let first = window_filter(&Filter::none(), mode, before_midnight).unwrap();
        let second = window_filter(&Filter::none(), mode, after_midnight).unwrap();

        assert_eq!(first.since, Some(local(2026, 9, 12, 0, 0)));
        assert_eq!(second.since, Some(local(2026, 9, 13, 0, 0)));
        assert_ne!(first.since, second.since);
    }

    #[test]
    fn window_filter_keeps_base_predicates() {
        let base = Filter {
            project: Some("proj".to_string()),
            source: Some("claude".to_string()),
            ..Default::default()
        };
        let got = window_filter(
            &base,
            Some(WindowMode::Calendar { days: 7 }),
            local(2026, 9, 12, 20, 20),
        )
        .unwrap();
        assert_eq!(got.project.as_deref(), Some("proj"));
        assert_eq!(got.source.as_deref(), Some("claude"));
        assert_eq!(got.since, Some(local(2026, 9, 6, 0, 0)));
    }
}

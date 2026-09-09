use chrono::{DateTime, Utc};
use std::time::Duration;

use crate::domain::message::Message;
use crate::domain::record::Record;

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
        if let Some(since) = self.since {
            if at.is_none_or(|t| t < since) {
                return false;
            }
        }
        if let Some(window) = self.last {
            let cutoff = Utc::now() - window;
            if at.is_none_or(|t| t < cutoff) {
                return false;
            }
        }
        if let Some(until) = self.until {
            if at.is_none_or(|t| t > until) {
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
}

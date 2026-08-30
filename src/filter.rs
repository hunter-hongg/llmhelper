use std::time::Duration;
use chrono::{DateTime, Utc};

use crate::domain::record::Record;

/// Pre-aggregation filters applied to a record set.
/// All predicates AND-combine; an empty filter passes everything through.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub since: Option<DateTime<Utc>>,
    pub last: Option<Duration>,
    pub project: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
}

impl Filter {
    pub fn none() -> Self {
        Self::default()
    }

    /// Check whether a single record satisfies this filter.
    pub fn matches(&self, r: &Record) -> bool {
        if let Some(since) = self.since {
            if r.started_at < since {
                return false;
            }
        }
        if let Some(window) = self.last {
            let cutoff = Utc::now() - window;
            if r.started_at < cutoff {
                return false;
            }
        }
        if let Some(proj) = &self.project {
            if !r.project.contains(proj.as_str()) {
                return false;
            }
        }
        if let Some(model) = &self.model {
            if !r.model.to_lowercase().contains(&model.to_lowercase()) {
                return false;
            }
        }
        if let Some(src) = &self.source {
            if r.source != *src {
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
        let r = vec![rec("a", "/p", "m", Utc::now()), rec("b", "/q", "n", Utc::now())];
        assert_eq!(Filter::none().apply(&r).len(), 2);
    }

    #[test]
    fn filter_source() {
        let r = vec![
            rec("claude", "/p", "m", Utc::now()),
            rec("opencode", "/p", "m", Utc::now()),
        ];
        let f = Filter { source: Some("claude".to_string()), ..Default::default() };
        assert_eq!(f.apply(&r).len(), 1);
        assert_eq!(f.apply(&r)[0].source, "claude");
    }

    #[test]
    fn filter_project_substring() {
        let r = vec![
            rec("a", "/home/user/proj-x", "m", Utc::now()),
            rec("a", "/other/y", "m", Utc::now()),
        ];
        let f = Filter { project: Some("proj".to_string()), ..Default::default() };
        assert_eq!(f.apply(&r).len(), 1);
    }

    #[test]
    fn filter_model_case_insensitive() {
        let r = vec![
            rec("a", "/p", "Big-Pickle", Utc::now()),
            rec("a", "/p", "Auto", Utc::now()),
        ];
        let f = Filter { model: Some("big".to_string()), ..Default::default() };
        assert_eq!(f.apply(&r).len(), 1);
        assert_eq!(f.apply(&r)[0].model, "Big-Pickle");
    }

    #[test]
    fn filter_since() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(30);
        let r = vec![
            rec("a", "/p", "m", old),
            rec("a", "/p", "m", now),
        ];
        let f = Filter { since: Some(now - chrono::Duration::days(7)), ..Default::default() };
        assert_eq!(f.apply(&r).len(), 1);
    }

    #[test]
    fn filter_last() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(60);
        let r = vec![
            rec("a", "/p", "m", old),
            rec("a", "/p", "m", now),
        ];
        let f = Filter { last: Some(Duration::from_secs(30 * 24 * 3600)), ..Default::default() };
        assert_eq!(f.apply(&r).len(), 1);
    }
}

use std::collections::BTreeMap;

use serde::Serialize;

use crate::domain::group::GroupBy;
use crate::domain::record::{Record, TokenBreakdown};
use crate::filter::Filter;

/// A single group within an aggregated result.
#[derive(Clone, Debug, Serialize)]
pub struct Group {
    pub key: String,
    pub source: String,
    pub sessions: usize,
    pub messages: usize,
    pub tokens: TokenBreakdown,
    pub cost: Option<f64>,
}

/// Complete aggregation result returned after filtering + grouping.
#[derive(Clone, Debug)]
pub struct AggregateResult {
    pub grand_totals: TokenBreakdown,
    pub grand_messages: usize,
    pub grand_sessions: usize,
    pub groups: Vec<Group>,
}

impl AggregateResult {
    /// Build an aggregate from raw records, a filter, and a grouping dimension.
    pub fn from_records(records: &[Record], filter: &Filter, group_by: GroupBy) -> Self {
        let filtered: Vec<&Record> = filter.apply(records);
        let mut groups: BTreeMap<String, GroupEntry> = BTreeMap::new();

        for r in &filtered {
            let key = match group_by {
                GroupBy::Source => r.source.clone(),
                GroupBy::Project => r.project.clone(),
                GroupBy::Model => r.model.clone(),
            };
            let entry = groups.entry(key).or_insert_with(|| GroupEntry {
                source: r.source.clone(),
                mixed_source: false,
                sessions: 0,
                messages: 0,
                tokens: TokenBreakdown::default(),
                cost: None,
            });
            entry.sessions += 1;
            entry.messages += r.message_count as usize;
            entry.tokens.add(&r.tokens);

            // Cost scoping: only sum within a single source.
            // Once a group has records from multiple sources, cost is permanently None.
            if entry.mixed_source {
                entry.cost = None;
            } else if let (Some(a), Some(b)) = (entry.cost, r.cost) {
                entry.cost = Some(a + b);
            } else if entry.cost.is_none() && r.cost.is_some() {
                if entry.source != r.source {
                    entry.mixed_source = true;
                    entry.cost = None;
                } else {
                    entry.cost = r.cost;
                }
            } else if entry.cost.is_some() && r.cost.is_none() {
                entry.mixed_source = true;
                entry.cost = None;
            }
        }

        let groups: Vec<Group> = groups
            .into_iter()
            .map(|(key, e)| Group {
                key,
                source: e.source,
                sessions: e.sessions,
                messages: e.messages,
                tokens: e.tokens,
                cost: e.cost,
            })
            .collect();

        let grand_tokens = TokenBreakdown::sum(&filtered);
        let grand_messages: usize = filtered.iter().map(|r| r.message_count as usize).sum();
        let grand_sessions = filtered.len();

        Self {
            grand_totals: grand_tokens,
            grand_messages,
            grand_sessions,
            groups,
        }
    }
}

/// Internal mutable accumulator during aggregation.
struct GroupEntry {
    source: String,
    mixed_source: bool,
    sessions: usize,
    messages: usize,
    tokens: TokenBreakdown,
    cost: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use chrono::Utc;

    fn rec(source: &str, project: &str, model: &str, cost: Option<f64>) -> Record {
        Record {
            session_id: format!("ses_{}", source),
            source: source.to_string(),
            project: project.to_string(),
            model: model.to_string(),
            agent: None,
            started_at: Utc::now(),
            ended_at: None,
            tokens: TokenBreakdown {
                input: 10,
                output: 5,
                reasoning: 1,
                cache_read: 2,
                cache_write: 3,
            },
            message_count: 2,
            cost,
        }
    }

    #[test]
    fn aggregate_groups_by_source() {
        let records = vec![
            rec("claude", "/p", "auto", None),
            rec("claude", "/p", "auto", None),
            rec("opencode", "/p", "big-pickle", Some(0.1)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        assert_eq!(agg.groups.len(), 2);
        let claude = agg.groups.iter().find(|g| g.key == "claude").unwrap();
        assert_eq!(claude.sessions, 2);
        assert_eq!(claude.tokens.input, 20);
        assert_eq!(claude.cost, None);
        let oc = agg.groups.iter().find(|g| g.key == "opencode").unwrap();
        assert_eq!(oc.sessions, 1);
        assert_eq!(oc.cost, Some(0.1));
    }

    #[test]
    fn aggregate_mixed_group_cost_is_none() {
        // Interleaved: opencode, claude, opencode — cost must be None.
        let records = vec![
            rec("opencode", "/shared", "m", Some(0.1)),
            rec("claude", "/shared", "m", None),
            rec("opencode", "/shared", "m", Some(0.2)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Project);
        let shared = agg.groups.iter().find(|g| g.key == "/shared").unwrap();
        assert_eq!(shared.cost, None, "mixed group must have cost=None");
    }

    #[test]
    fn aggregate_same_source_costs_sum() {
        let records = vec![
            rec("opencode", "/p", "m", Some(0.1)),
            rec("opencode", "/p", "m", Some(0.2)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let oc = agg.groups.iter().find(|g| g.key == "opencode").unwrap();
        assert!((oc.cost.unwrap() - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_groups_by_model() {
        let records = vec![
            rec("claude", "/p", "auto", None),
            rec("opencode", "/p", "auto", Some(0.01)),
            rec("opencode", "/p", "big-pickle", Some(0.02)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Model);
        let auto = agg.groups.iter().find(|g| g.key == "auto").unwrap();
        assert_eq!(auto.sessions, 2);
        let bp = agg.groups.iter().find(|g| g.key == "big-pickle").unwrap();
        assert_eq!(bp.sessions, 1);
        assert_eq!(bp.cost, Some(0.02));
    }

    #[test]
    fn filter_then_aggregate() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(100);
        let records = vec![
            rec("claude", "/p", "auto", None),
            rec("opencode", "/p", "big-pickle", Some(0.1)),
        ];
        let f = Filter {
            since: Some(old + chrono::Duration::days(1)),
            ..Default::default()
        };
        let agg = AggregateResult::from_records(&records, &f, GroupBy::Source);
        assert_eq!(agg.groups.iter().filter(|g| g.key == "claude").count(), 1);
    }
}

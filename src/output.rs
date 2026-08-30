use std::io::Write;

use serde::Serialize;

use crate::aggregator::Group;
use crate::source::SourceStatus;

#[derive(Clone, Debug, Serialize)]
struct SourceInfo {
    name: String,
    records: usize,
    status: String,
}

#[derive(Clone, Debug, Serialize)]
struct JsonPayload {
    sources: Vec<SourceInfo>,
    group_by: String,
    groups: Vec<Group>,
}

pub struct OutputRenderer;

impl OutputRenderer {
    pub fn json<W: Write>(
        &self,
        groups: &[Group],
        source_statuses: &[SourceStatus],
        group_by: &str,
        out: &mut W,
    ) -> anyhow::Result<()> {
        let sources: Vec<SourceInfo> = source_statuses
            .iter()
            .map(|s| SourceInfo {
                name: s.name.clone(),
                records: s.record_count,
                status: match &s.error {
                    None => "ok".to_string(),
                    Some(e) => format!("{}", e),
                },
            })
            .collect();
        let payload = JsonPayload {
            sources,
            group_by: group_by.to_string(),
            groups: groups.to_vec(),
        };
        serde_json::to_writer_pretty(out, &payload)?;
        Ok(())
    }

    pub fn csv<W: Write>(&self, groups: &[Group], out: &mut W) -> anyhow::Result<()> {
        let mut w = csv::Writer::from_writer(out);
        w.write_record(&[
            "group_key",
            "source",
            "sessions",
            "messages",
            "input",
            "output",
            "reasoning",
            "cache_read",
            "cache_write",
            "cost",
        ])?;
        for g in groups {
            w.write_record(&[
                &g.key,
                &g.source,
                &g.sessions.to_string(),
                &g.messages.to_string(),
                &g.tokens.input.to_string(),
                &g.tokens.output.to_string(),
                &g.tokens.reasoning.to_string(),
                &g.tokens.cache_read.to_string(),
                &g.tokens.cache_write.to_string(),
                &g.cost
                    .map(|c| format!("{:.6}", c))
                    .unwrap_or_default(),
            ])?;
        }
        w.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::AggregateResult;
    use crate::domain::GroupBy;
    use crate::domain::record::{Record, TokenBreakdown};
    use crate::filter::Filter;
    use chrono::Utc;

    fn fake_record(source: &str, project: &str, model: &str, cost: Option<f64>) -> Record {
        Record {
            session_id: format!("ses_{}", source),
            source: source.to_string(),
            project: project.to_string(),
            model: model.to_string(),
            agent: None,
            started_at: Utc::now(),
            ended_at: None,
            tokens: TokenBreakdown::default(),
            message_count: 1,
            cost,
        }
    }

    #[test]
    fn csv_emits_header_and_rows() {
        let records = vec![
            fake_record("claude", "/proj/a", "auto", None),
            fake_record("opencode", "/proj/a", "big-pickle", Some(0.12)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let mut buf = Vec::new();
        OutputRenderer.csv(&agg.groups, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("group_key,source,sessions,messages"));
        assert!(text.contains("claude"));
        assert!(text.contains("opencode"));
    }

    #[test]
    fn csv_cost_blank_for_claude() {
        let records = vec![fake_record("claude", "/p", "auto", None)];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let mut buf = Vec::new();
        OutputRenderer.csv(&agg.groups, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);
        let parts: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(parts[9], "");
    }

    #[test]
    fn json_output_structure() {
        let records = vec![
            fake_record("claude", "/proj/a", "auto", None),
            fake_record("opencode", "/proj/a", "big-pickle", Some(0.12)),
        ];
        let agg = AggregateResult::from_records(&records, &Filter::none(), GroupBy::Source);
        let statuses = vec![
            SourceStatus {
                name: "claude".to_string(),
                record_count: 1,
                error: None,
            },
            SourceStatus {
                name: "opencode".to_string(),
                record_count: 1,
                error: None,
            },
        ];
        let mut buf = Vec::new();
        let renderer = OutputRenderer;
        renderer.json(&agg.groups, &statuses, "source", &mut buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(parsed.get("sources").is_some());
        assert!(parsed.get("groups").is_some());
        assert_eq!(parsed["group_by"], "source");
        let group_keys: Vec<&str> = parsed["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["key"].as_str().unwrap())
            .collect();
        assert!(group_keys.contains(&"claude"));
        assert!(group_keys.contains(&"opencode"));
    }
}

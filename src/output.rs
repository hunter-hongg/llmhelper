use std::io::Write;

use serde::Serialize;

use crate::domain::record::TokenBreakdown;

/// A single row of grouped aggregation output.
#[derive(Clone, Debug, Serialize)]
pub struct GroupRow {
    pub key: String,
    pub source: String,
    pub sessions: usize,
    pub messages: usize,
    pub tokens: TokenBreakdown,
    pub cost: Option<f64>,
}

impl GroupRow {
    pub fn to_json_map(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "key".to_string(),
            serde_json::Value::String(self.key.clone()),
        );
        m.insert(
            "source".to_string(),
            serde_json::Value::String(self.source.clone()),
        );
        m.insert(
            "sessions".to_string(),
            serde_json::Value::Number(self.sessions.into()),
        );
        m.insert(
            "messages".to_string(),
            serde_json::Value::Number(self.messages.into()),
        );
        m.insert(
            "tokens".to_string(),
            serde_json::to_value(&self.tokens).unwrap(),
        );
        m.insert(
            "cost".to_string(),
            match self.cost {
                Some(c) => serde_json::Value::Number(
                    serde_json::Number::from_f64(c).unwrap_or(serde_json::Number::from(0)),
                ),
                None => serde_json::Value::Null,
            },
        );
        m
    }
}

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
    groups: Vec<serde_json::Value>,
}

pub struct OutputRenderer;

impl OutputRenderer {
    pub fn json<W: Write>(
        &self,
        groups: &[GroupRow],
        source_statuses: &[crate::source::SourceStatus],
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
            groups: groups
                .iter()
                .map(|g| serde_json::Value::Object(g.to_json_map()))
                .collect(),
        };
        serde_json::to_writer_pretty(out, &payload)?;
        Ok(())
    }

    pub fn csv<W: Write>(&self, groups: &[GroupRow], out: &mut W) -> anyhow::Result<()> {
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
    use crate::domain::record::Record;
    use crate::domain::GroupBy;
    use crate::filter::Filter;
    use crate::aggregator::AggregateResult;
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
        let agg = AggregateResult::from_records(
            &records,
            &Filter::none(),
            GroupBy::Source,
        );
        let rows: Vec<GroupRow> = agg.groups.iter().map(|g| GroupRow { key: g.key.clone(), source: g.source.clone(), sessions: g.sessions, messages: g.messages, tokens: g.tokens.clone(), cost: g.cost }).collect();
        let mut buf = Vec::new();
        OutputRenderer.csv(&rows, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("group_key,source,sessions,messages"));
        assert!(text.contains("claude"));
        assert!(text.contains("opencode"));
    }

    #[test]
    fn csv_cost_blank_for_claude() {
        let records = vec![fake_record("claude", "/p", "auto", None)];
        let agg = AggregateResult::from_records(
            &records,
            &Filter::none(),
            GroupBy::Source,
        );
        let rows: Vec<GroupRow> = agg.groups.iter().map(|g| GroupRow { key: g.key.clone(), source: g.source.clone(), sessions: g.sessions, messages: g.messages, tokens: g.tokens.clone(), cost: g.cost }).collect();
        let mut buf = Vec::new();
        OutputRenderer.csv(&rows, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);
        let parts: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(parts[9], ""); // cost column
    }
}

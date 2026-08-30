use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Five-part token accounting for a single Session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenBreakdown {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenBreakdown {
    /// Saturating-add another breakdown into this one.
    pub fn add(&mut self, other: &TokenBreakdown) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// Sum a slice of records' token breakdowns into a new one.
    pub fn sum(records: &[&Record]) -> Self {
        let mut acc = Self::default();
        for r in records {
            acc.add(&r.tokens);
        }
        acc
    }
}

/// Normalized session record emitted by every Source.
///
/// `cost` is `None` for sources that do not record spend (Claude Code).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub session_id: String,
    pub source: String,
    pub project: String,
    pub model: String,
    pub agent: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub tokens: TokenBreakdown,
    pub message_count: u32,
    pub cost: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_breakdown_adds_correctly() {
        let a = TokenBreakdown {
            input: 100, output: 50, reasoning: 10,
            cache_read: 20, cache_write: 5,
        };
        let b = TokenBreakdown {
            input: 200, output: 100, reasoning: 0,
            cache_read: 10, cache_write: 3,
        };
        let mut result = a.clone();
        result.add(&b);
        assert_eq!(result.input, 300);
        assert_eq!(result.output, 150);
        assert_eq!(result.reasoning, 10);
        assert_eq!(result.cache_read, 30);
        assert_eq!(result.cache_write, 8);
    }

    #[test]
    fn token_breakdown_sum_with_empty() {
        assert_eq!(TokenBreakdown::sum(&[]), TokenBreakdown::default());
    }

    #[test]
    fn token_breakdown_serialize_roundtrip() {
        let t = TokenBreakdown {
            input: 1, output: 2, reasoning: 3,
            cache_read: 4, cache_write: 5,
        };
        let s = serde_json::to_string(&t).unwrap();
        let decoded: TokenBreakdown = serde_json::from_str(&s).unwrap();
        assert_eq!(t, decoded);
    }
}

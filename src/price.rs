//! Token-to-Cost conversion via the `[price.<model>]` config table.
//!
//! A [`ModelPrice`] is one model's per-million-token rates. Cost is
//! `Σ(tokens × rate) / 1_000_000` — the same currency every Source reports,
//! so a budget can judge a priced turn exactly as it judges OpenCode's
//! recorded `cost`. A model with no table entry prices to `None`: absent
//! pricing is not free, it is unmeasured.

use std::collections::BTreeMap;

use crate::domain::record::TokenBreakdown;

/// One model's per-million-token rates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    pub input_per_mtoken: f64,
    pub output_per_mtoken: f64,
    /// Defaults to the input rate when the config omits it.
    pub cache_read_per_mtoken: f64,
    /// Defaults to the input rate when the config omits it.
    pub cache_write_per_mtoken: f64,
}

impl ModelPrice {
    /// The Cost of one token breakdown under this price.
    pub fn cost(&self, tokens: &TokenBreakdown) -> f64 {
        (tokens.input as f64 * self.input_per_mtoken
            + tokens.output as f64 * self.output_per_mtoken
            + tokens.cache_read as f64 * self.cache_read_per_mtoken
            + tokens.cache_write as f64 * self.cache_write_per_mtoken)
            / 1_000_000.0
    }
}

/// Rates keyed by the exact model string a provider echoes. Sorted by key so
/// two runs over the same config produce byte-identical output.
pub type PriceTable = BTreeMap<String, ModelPrice>;

/// The Cost of `tokens` for `model`, or `None` when the model is unpriced.
/// Exact match only: a versioned echo (`gpt-4o-2024-08-06`) needs its own
/// entry, per spec 0027's Out of Scope.
pub fn cost_for(table: &PriceTable, model: &str, tokens: &TokenBreakdown) -> Option<f64> {
    table.get(model).map(|price| price.cost(tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenBreakdown {
        TokenBreakdown {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    fn price() -> ModelPrice {
        ModelPrice {
            input_per_mtoken: 2.5,
            output_per_mtoken: 10.0,
            cache_read_per_mtoken: 0.3,
            cache_write_per_mtoken: 1.25,
        }
    }

    #[test]
    fn cost_is_the_sum_of_tokens_times_rates_per_million() {
        let c = price().cost(&tokens(1_000_000, 500_000, 2_000_000, 400_000));
        // 1M×2.5 + 0.5M×10 + 2M×0.3 + 0.4M×1.25 = 2.5 + 5 + 0.6 + 0.5
        assert!((c - 8.6).abs() < 1e-9, "cost was {}", c);
    }

    #[test]
    fn zero_tokens_is_measured_zero_not_absent() {
        // Some(0.0) is a real spend; None means "unpriced". A priced model
        // with no tokens must land `Under`, not `NotMeasured`.
        let table = PriceTable::from([("m".to_string(), price())]);
        assert_eq!(cost_for(&table, "m", &tokens(0, 0, 0, 0)), Some(0.0));
    }

    #[test]
    fn unknown_model_is_unpriced_not_zero() {
        let table = PriceTable::from([("gpt-4".to_string(), price())]);
        assert_eq!(cost_for(&table, "gpt-4o", &tokens(10, 10, 0, 0)), None);
    }

    #[test]
    fn model_match_is_exact_versioned_suffix_and_all() {
        let table = PriceTable::from([("gpt-4o".to_string(), price())]);
        // A versioned echo must NOT fall back to the base model's rate.
        assert_eq!(
            cost_for(&table, "gpt-4o-2024-08-06", &tokens(1, 0, 0, 0)),
            None
        );
        assert!(cost_for(&table, "gpt-4o", &tokens(1, 0, 0, 0)).is_some());
    }

    #[test]
    fn empty_table_prices_nothing() {
        assert_eq!(
            cost_for(&PriceTable::new(), "m", &tokens(10, 10, 10, 10)),
            None
        );
    }
}

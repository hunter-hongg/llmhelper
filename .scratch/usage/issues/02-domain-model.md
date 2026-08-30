# 02 — Domain model: Record + TokenBreakdown

**What to build:** The canonical normalized data types every layer speaks. A `Record` (per-session normalized entity) carrying session id, source name, project, model, optional agent, start/end timestamps, a `TokenBreakdown` (input/output/reasoning/cache_read/cache_write as `u64`), message count, and an optional cost (`f64`). Cost is the only optional field — `None` for sources that don't record it. Types derive `Clone`, `Debug`, `Serialize` (for `--json` output) and are unit-testable in isolation.

**Blocked by:** None — can start immediately. (Pairs naturally with 01 but has no hard dependency; both can start in parallel.)

**Status:** ready-for-agent

- [ ] `Record` and `TokenBreakdown` exist with the exact fields from the spec (no extra source-specific fields)
- [ ] `TokenBreakdown` has a zero value and an saturating-add `+=` helper so adapters and the aggregator can sum token blocks without overflow
- [ ] `cost: Option<f64>` is the only optional field; everything else required
- [ ] Both types derive `Clone`, `Debug`, and `Serialize` (`serde`)
- [ ] A unit test constructs a `Record`, asserts `Clone` equality and that `TokenBreakdown` addition sums correctly

# 08 — Aggregator (group by Source / Project / Model)

**What to build:** The aggregation layer that turns filtered `Vec<Record>` into grouped summaries: grand totals plus a list of groups keyed by Source, Project, or Model. Each group carries session count, message count, summed `TokenBreakdown`, and cost (summed only within the group — and cost stays `None` for Claude groups since every record there is `None`; cross-source totals never sum cost). This is the data the JSON/CSV/TUI tickets all render.

**Blocked by:** 02, 07 — consumes `Record`s after filtering.

**Status:** ready-for-agent

- [ ] An aggregator takes `Vec<Record>` + a `GroupBy` enum (Source | Project | Model) and returns `{ totals, groups: Vec<GroupSummary> }`
- [ ] `GroupSummary` has key, session count, message count, `TokenBreakdown` sum, and `Option<f64>` cost (summed within group only)
- [ ] Grand `totals` computed independently of grouping; token sums saturate-safe
- [ ] Cost is `None` in any group containing only Claude records; `Some` where OpenCode records contribute (never summed across sources — only within a same-source group)
- [ ] A unit test with mixed Claude/OpenCode fixtures asserts per-group token sums and that a Claude group's cost is `None`

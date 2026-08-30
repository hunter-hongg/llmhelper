# 07 — Filter combinator

**What to build:** A pure filter layer applied to `Vec<Record>` before aggregation. Filters: `--since` (started_at ≥ RFC3339), `--last` (started_at ≥ now − duration like `7d`/`4h`), `--project` (project path substring), `--model` (model substring, case-insensitive), `--source` (`claude`|`opencode`). All filters AND-combine; an empty filter set passes everything through. `--since` and `--last` are mutually exclusive (validated at the CLI layer in 13, but the combinator accepts whichever are set).

**Blocked by:** 02 — operates on `Record`s.

**Status:** ready-for-agent

- [ ] A `Filter` struct with optional fields for each flag; `apply(records) -> Vec<Record>` AND-combines them
- [ ] `--since` compares `started_at >= since`; `--last` computes `now - duration` and compares `>=`; duration parses `Nd`/`Nh`/`Nm`
- [ ] `--project` substring, `--model` case-insensitive substring, `--source` exact match against `record.source`
- [ ] A unit test asserts: each filter alone retains the right subset; all filters together AND correctly; empty filter retains all

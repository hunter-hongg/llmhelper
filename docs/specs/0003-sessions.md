---
id: 0003
title: "sessions — drill down into individual Agent Sessions"
status: ready-for-agent
created: 2026-09-05
triage: ready-for-agent
---

## Problem Statement

`usage` provides aggregated token consumption grouped by Source / Project / Model, and `diff` compares two sliding windows. Neither command allows an agent user to locate, inspect, or filter a single Session record. The user cannot answer "which session id produced this spike?" or "show me all sessions for project X in the last 3 days" without exporting JSON and processing it externally. There is no native drill-down seam in the CLI.

## Solution

A new subcommand `sessions` that lists normalized Session records from all configured Sources, applies the same filter set as `usage`, and renders a tabular list plus optional detail view. The command reuses the existing Source registry, Record normalization, and Filter pipeline, and renders via terminal table, `--json`, and `--csv`. It is a read-only, one-shot listing operation with no live refresh.

## User Stories

1. As an agent user, I want to list all Sessions across Sources, so that I can see what Sessions exist locally.
2. As an agent user, I want to filter Sessions by `--last <duration>`, so that I can focus on recent activity.
3. As an agent user, I want to filter Sessions by `--since <RFC3339>`, so that I can inspect a specific time range.
4. As an agent user, I want to filter Sessions by `--project <substring>`, so that I can isolate a repository.
5. As an agent user, I want to filter Sessions by `--model <substring>`, so that I can find Sessions using a specific model.
6. As an agent user, I want to filter Sessions by `--source <claude|opencode|omp|kilo>`, so that I can narrow to one agent.
7. As an agent user, I want Sessions sorted by `started_at` descending by default, so that the most recent Sessions appear first.
8. As an agent user, I want a summary column showing sessions count, messages, input/output/cache tokens and optional cost, so that I can scan Session health at a glance.
9. As an agent user, I want to view Session detail with `--detail <session-id>`, so that I can see full Record fields.
10. As an agent user, I want Session ids to be the source-native id, so that I can correlate with original transcripts.
11. As a power user, I want `--limit <n>` to cap the number of listed Sessions, so that large result sets stay manageable.
12. As a power user, I want `--offset <n>` for pagination, so that I can page through results.
13. As a script author, I want `--json` output, so that I can pipe Session listings into jq or other tools.
14. As a script author, I want `--csv` output, so that I can import Session listings into spreadsheets.
15. As a config-driven user, I want `sessions` to respect `~/.config/llmhelper/config.toml` source overrides, so that defaults are shared with `usage`/`diff`.
16. As a multi-machine user, I want CLI overrides `--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db` on `sessions`, so that it works with non-default paths.
17. As a user, I want missing Sources to degrade gracefully with a SourceStatus error, so that partial data does not abort the listing.
18. As a user, I want cost to be shown only when the Source records it, so that I do not see misleading zeros.
19. As a user, I want token counts formatted with K/M/B magnitude, so that large numbers are readable.
20. As a user, I want the listing to preserve the canonical TokenBreakdown fields input/output/cache_read/cache_write with reasoning folded into output, so that terminology stays consistent.
21. As a developer, I want `sessions` to reuse the existing Filter and Record types, so that no new domain types are introduced.
22. As a developer, I want `sessions` to be testable via the existing `--json` CLI seam, so that integration tests remain uniform.

## Implementation Decisions

- New CLI enum variant `Command::Sessions(SessionsArgs)` with flags: `last`, `since`, `project`, `model`, `source`, `limit`, `offset`, `detail`, `json`, `csv`, plus source path overrides shared with `usage`.
- Validation: `--last` and `--since` are mutually exclusive; `--json` and `--csv` are mutually exclusive; `--detail` requires a session id.
- Data flow: `discover_sources(config)` → `registry.load_all()` → `Filter::matches(record)` → sort by `started_at` desc → apply `limit/offset` → render.
- Detail mode renders a single Record with all fields: session_id, source, project, model, agent, started_at, ended_at, message_count, tokens breakdown, cost.
- Listing mode columns: source, project, model, started_at, ended_at, sessions=1, messages, input, output, cache_read, cache_write, cost.
- Grouping is not performed; `sessions` is a flat list. Grouping remains the responsibility of `usage`.
- Cost handling follows ADR-0001: cost is shown per Source, never summed across Sources; `None` is rendered as empty.
- Token formatting uses the shared magnitude formatter from `usage` to keep K/M/B display consistent.
- No TUI / live refresh for `sessions`; it is a one-shot command.
- The `Filter` type is reused as-is; no new filter semantics are added.
- `SessionsArgs` mirrors `UsageArgs` for temporal and path filters to keep UX consistent.

## Testing Decisions

- Good test = external behavior only: given fixture source directories and CLI flags, the emitted `--json` output matches expected Sessions list and field values.
- Prior art: `tests/integration.rs` for `usage` and `diff` drives the binary with `--json` against `tests/fixtures/claude/`, `opencode/`, `omp/`, `kilo/`.
- Seam: single highest seam = CLI `--json` output. No new unit seams. The same fixture strategy is reused.
- Assertions cover:
  * total Session count matches fixture sum
  * `--source` filter returns only that Source's Sessions
  * `--project` substring filter matches decoded project paths
  * `--last` / `--since` time window filtering
  * `--limit` / `--offset` pagination
  * `--detail <id>` returns a single Record with correct tokens and cost
  * cost is `null` for Claude, present for OpenCode/OMP/Kilo where non-zero
  * model values are raw and normalized per Source rules
  * sorting by `started_at` desc
- No tests for internal rendering, TUI, or registry internals.

## Out of Scope

- Live refresh / TUI for `sessions`
- Editing or deleting Sessions
- Cross-Source cost aggregation
- Session content export, transcript retrieval, or message-level listing
- Creating new Sessions or writing data back to Sources
- Calendar-aligned windows or historical arbitrary date pairs; only simple `since`/`last` filters
- Grouping or aggregation; that is `usage`'s responsibility
- Interactive pagination in terminal; pagination is flag-based

## Further Notes

- The Session list provides the drill-down missing between `usage` aggregation and raw source files.
- Future `report` features can compose `sessions` + `usage` data.
- The spec respects domain glossary: Usage, Source, Project, Cost, TokenBreakdown, Model, Session.
- ADR-0001 source-scoped cost and ADR-0004 raw model value are preserved.

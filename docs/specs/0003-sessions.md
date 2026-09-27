---
id: 0003
title: "sessions — TUI Session inventory with Enter detail"
status: done
created: 2026-09-05
updated: 2026-09-25
triage: done
---

## Problem Statement

`sessions` is the flat Session inventory for this CLI: it lists normalized Session records across all configured Sources. `usage` answers aggregate questions, and its group-to-Session drill-down is intentionally contextual: it answers "which Sessions make up this selected group?" It does not answer "show me every Session matching these filters, newest first, across all groups."

The current `sessions` implementation already defaults to a TUI and already advertises `Enter` in the footer, but pressing `Enter` does nothing. This leaves the command in an inconsistent state: the user is told a Session detail interaction exists, but the UI does not provide one. At the same time, the command-level responsibilities remain clear: `usage` owns Usage aggregation, `diff` owns window comparison, and `sessions` owns flat Session listing, lookup, and export.

This spec updates the `sessions` contract so the TUI is explicitly part of the default command experience and so `Enter` actually opens a detail view for the selected Session.

## Solution

Make `sessions` a TUI-first Session inventory command with an interactive detail view.

In the default TUI, Sessions are listed as a flat table sorted by `started_at` descending. Selecting a Session and pressing `Enter` replaces the list with a detail view for that one Session. The detail view shows the Session's source-native id, Source, Project, raw Model, optional agent, start/end times, message count, canonical Token Breakdown, and source-scoped Cost where available. Pressing `Esc` returns to the list while preserving the previous list selection. `q` quits from either view. `r` refreshes data, and background refresh keeps both the list and any open detail view synchronized.

The non-TUI command paths remain unchanged: `--json`, `--csv`, and `--detail <session-id>` continue to provide scriptable one-shot output for automation and exact lookup. Pagination flags remain available to those non-TUI paths. `sessions` does not introduce grouping or aggregation; that responsibility remains with `usage`.

## User Stories

1. As an agent user, I want `sessions` to open a TUI by default, so that Session inventory matches the interactive experience of the other commands.
2. As an agent user, I want to list all Sessions across Sources, so that I can see what Sessions exist locally.
3. As an agent user, I want Sessions sorted by `started_at` descending, so that the most recent Sessions appear first.
4. As an agent user, I want to see the Session count in the header, so that I can tell whether my filters matched data.
5. As an agent user, I want to see each Source's loaded record count and error status, so that missing Sources do not look like empty data.
6. As an agent user, I want to see each Session's Source, Project, Model, start time, end time, message count, and tokens, so that I can scan Session health at a glance.
7. As an agent user, I want token counts formatted with K/M/B magnitude, so that large numbers remain readable.
8. As an agent user, I want Cost shown only when the Source records it, so that I do not interpret missing spend data as zero.
9. As an agent user, I want to navigate the Session list with arrow keys, so that the control is discoverable.
10. As an agent user, I want to navigate the Session list with `j` and `k`, so that keyboard-only navigation is fast.
11. As an agent user, I want the selected row to scroll into view, so that I never lose track of the current Session.
12. As an agent user, I want to press `Enter` on the selected Session, so that I can inspect that Session without leaving the command.
13. As an agent user, I want the Session detail view to show the source-native Session id, so that I can correlate it with original transcripts.
14. As an agent user, I want the Session detail view to show Source, Project, and raw Model, so that the detail row is interpretable even outside its list row.
15. As an agent user, I want the Session detail view to show the optional agent field when present, so that agent-level activity is visible without inventing new semantics.
16. As an agent user, I want the Session detail view to show start and end times, so that I can understand the Session's active window.
17. As an agent user, I want the Session detail view to show message count, so that activity level is visible.
18. As an agent user, I want the Session detail view to show input, output, cache-read, and cache-write tokens, so that the canonical Token Breakdown is visible.
19. As an agent user, I want output tokens to include reasoning tokens in Session detail, so that detail matches aggregate accounting.
20. As an agent user, I want Cost shown only for Sessions whose Source records it, so that source-scoped Cost behavior is preserved.
21. As an agent user, I want the footer to advertise `Enter` while in the list view, so that the detail interaction is discoverable.
22. As an agent user, I want `Esc` to return from Session detail to the Session list, so that drill-down is non-destructive.
23. As an agent user, I want the previous list selection to be preserved when I return from detail, so that I can reopen the same Session quickly.
24. As an agent user, I want the footer in detail to advertise `Esc`, so that the return control is discoverable.
25. As an agent user, I want `q` to quit from either the list or the detail view, so that exit behavior is predictable.
26. As an agent user, I want `Enter` to be a no-op when no Session is selected, so that an empty state does not trigger an error.
27. As an agent user, I want navigation in the detail view to remain harmless or disabled, so that arrow keys do not accidentally navigate through unrelated records.
28. As an agent user, I want `r` to refresh while in the list, so that newly written Sessions appear without quitting.
29. As an agent user, I want `r` to refresh while in the detail view, so that the detail row reflects current source data.
30. As an agent user, I want an open detail view to remain open when the selected Session id still exists after refresh, so that refresh does not unexpectedly close my inspection.
31. As an agent user, I want an open detail view to close when the selected Session id disappears after refresh, so that the UI does not show a record that no longer matches the current data.
32. As an agent user, I want the list selection to clamp into bounds after refresh when Session counts shrink, so that the UI does not point at a nonexistent row.
33. As an agent user, I want background refresh to update the list while I browse, so that new Sessions appear without manual refresh.
34. As an agent user, I want background refresh to keep an open detail row synchronized, so that stale detail data does not linger.
35. As an agent user, I want background refresh to close a missing detail Session, so that live data does not present a stale record as current.
36. As an agent user, I want `sessions` to respect `--last <duration>`, so that I can focus on recent activity.
37. As an agent user, I want `sessions` to respect `--since <RFC3339>`, so that I can inspect a specific time range.
38. As an agent user, I want `--last` and `--since` to be mutually exclusive, so that overlapping time filters do not produce ambiguous results.
39. As an agent user, I want to filter Sessions by `--project <substring>`, so that I can isolate a repository.
40. As an agent user, I want to filter Sessions by `--model <substring>`, so that I can find Sessions using a specific model.
41. As an agent user, I want to filter Sessions by `--source <source>`, so that I can narrow to one agent.
42. As a power user, I want `--limit <n>` to cap the number of listed Sessions in non-TUI output, so that large result sets stay manageable.
43. As a power user, I want `--offset <n>` in non-TUI output, so that I can page through results.
44. As a power user, I want `--detail <session-id>` to fetch one Session without opening the TUI, so that I can correlate an id found elsewhere.
45. As a power user, I want `--detail <session-id>` to fail clearly when the id is absent, so that automation can distinguish lookup misses.
46. As a power user, I want `--json` output, so that I can pipe Session listings or one Session detail into `jq` or other tools.
47. As a power user, I want `--csv` output, so that I can import Session listings into spreadsheets.
48. As a power user, I want `--json` and `--csv` to be mutually exclusive, so that output format is unambiguous.
49. As a config-driven user, I want `sessions` to respect configured source path overrides, so that defaults are shared with `usage` and `diff`.
50. As a multi-machine user, I want CLI overrides for Claude, OpenCode, OMP, and Kilo data locations on `sessions`, so that it works with non-default paths.
51. As a user, I want missing or unreadable Sources to degrade gracefully with a SourceStatus error, so that partial data does not abort the Session inventory.
52. As a user, I want source errors shown in the TUI, so that I can diagnose missing data without leaving the command.
53. As a user, I want Session detail to show missing end time clearly, so that an unfinished Session is not confused with missing data.
54. As a user, I want Session detail to distinguish unavailable Cost from zero, so that Sources without spend data are not misrepresented.
55. As a power user, I want the raw Model value to remain unresolved in detail, so that routing aliases are not guessed.
56. As a developer, I want `sessions` to reuse the existing Source, Filter, and Record pipeline, so that no new domain model is introduced.
57. As a developer, I want `sessions` to keep its data model flat, so that grouping responsibilities remain with `usage`.
58. As a developer, I want Session detail state to be modeled as TUI state, so that it can be tested without driving a real terminal.
59. As a developer, I want the TUI state methods to preserve or close detail deterministically during refresh, so that concurrent live updates remain predictable.
60. As a developer, I want existing JSON, CSV, pagination, and detail integration tests to remain valid, so that adding TUI detail does not regress scriptable behavior.

## Implementation Decisions

- `sessions` keeps a flat Session list. No grouping is introduced.
- The default command mode is TUI. `--json`, `--csv`, and `--detail <session-id>` continue to select non-TUI output paths.
- The TUI has two views: list and detail.
- The list view shows all filtered Session records, sorted by `started_at` descending.
- The detail view is keyed by source-native Session id.
- The detail record is a normalized Record snapshot selected from the current filtered list.
- Pressing `Enter` in the list view with a selected record opens the detail view.
- Pressing `Enter` with no list selection, an empty list, or while already in detail is a no-op.
- Pressing `Esc` in detail returns to list and preserves the list selection.
- Pressing `Esc` in list quits, matching the existing sessions behavior.
- Pressing `q` quits from either view.
- Arrow and `j`/`k` navigation belongs to the list view. In detail, navigation should not change an unrelated Session.
- Manual refresh reloads sources, reapplies the filter, resorts the list, and updates source statuses in either view.
- Background refresh uses the existing bounded channel and the same update rules as manual refresh.
- During refresh, the list selection is clamped into the new list bounds.
- If an open detail Session id still exists after refresh, the detail view remains open and its Record is replaced with the refreshed Record.
- If the open detail Session id disappears after refresh, the detail view closes and the user returns to list.
- The list table continues to show Source, Project, Model, Started, Ended, Messages, Input, Output, Cache Read, Cache Write, and Cost.
- The detail view adds the fields that are not fully visible in the list, especially Session id and optional agent.
- Cost remains per Session and source-scoped. It is never summed across Sources.
- Model remains the raw value recorded by the Source. Routing aliases such as `auto` are not resolved.
- Reasoning tokens continue to be folded into output before rendering.
- Non-TUI JSON/CSV schemas are unchanged.
- `--limit`, `--offset`, and `--detail` validation remains unchanged.
- The TUI footer is view-aware: list advertises `Enter`, detail advertises `Esc`.
- The command remains read-only. No Session editing, deletion, transcript export, or source write-back is added.

## Testing Decisions

- The highest meaningful seam for the interactive detail behavior is the TUI state API, following the pattern established for `usage` drill-down. A real terminal should not be required to test state transitions.
- Add focused unit tests for sessions TUI state:
  - `Enter` with a selected Session opens detail for that exact Session id.
  - `Enter` without a selection is a no-op.
  - `Enter` with an empty list is a no-op.
  - `Esc` returns to list while preserving the list selection.
  - Refresh clamps the list selection into bounds when records shrink.
  - Refresh preserves an open detail view when the Session id still exists and replaces its data.
  - Refresh closes an open detail view when the Session id disappears.
  - Detail navigation does not alter the selected Session record.
- Existing integration tests for `sessions --json`, filters, pagination, `--detail`, and sorting remain valid and must continue to pass.
- No new Source trait tests are required because no adapter behavior changes.
- Rendering layout should be verified manually in a terminal after unit tests pass, including narrow and wide widths.

## Out of Scope

- Message-level transcript contents or transcript export.
- Editing, deleting, creating, or writing back Session data.
- Grouping or aggregation inside `sessions`.
- Cross-Source Cost totals in list or detail.
- Resolving model routing aliases.
- New JSON fields or CSV columns.
- New CLI flags beyond the existing sessions arguments.
- TUI pagination flags or interactive date filters.
- Persistent memory of the last opened Session across separate invocations.
- Nested drill-down from Session detail into deeper source-specific structures.
- A separate `usage` Session export schema.

## Further Notes

- The Session list in `usage` remains an aggregate-to-Session bridge for the currently selected group.
- `sessions` remains the flat, command-level Session inventory and the scriptable Session record interface.
- This update intentionally supersedes the earlier decision that `sessions` had no TUI or live refresh. The current project convention is to default CLI commands to TUI unless the user explicitly selects JSON or CSV output.
- The detail view is not a transcript viewer. It is a normalized Record detail view for the selected Session.

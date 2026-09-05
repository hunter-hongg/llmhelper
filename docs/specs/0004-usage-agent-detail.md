---
id: 0004
title: "usage — Agent drill-down detail view"
status: ready-for-agent
created: 2026-09-05
triage: ready-for-agent
---

## Problem Statement

The `usage` TUI shows aggregate Usage rows grouped by Source, Project, or Model, but it stops at the aggregate. When the user selects a Source row — the default grouping, where each row is an Agent such as Claude Code, OpenCode, OMP, or Kilo Code — there is no way to see which individual Sessions make up that Agent's totals. The user cannot answer "which Sessions belong to this Agent?", "which Session drove this spike?", or "what model/project did this Agent actually run in?" without leaving the TUI and re-running a command.

## Solution

Add an interactive drill-down view to the existing `usage` TUI. With a group row selected, the user presses `Enter`; the group table is replaced by a Session detail table for the currently selected group. The detail table lists every filtered Session in that group, sorted by `started_at` descending, and shows the Session identifiers, timestamps, project, model, message count, canonical Token Breakdown values, and source-scoped Cost where available. `Esc` returns to the group table while preserving the group selection; `q` still quits; `j`/`k` and arrows navigate detail rows; `r` refreshes both views; live background refresh updates the detail data without losing the selected group.

## User Stories

1. As an Agent user, I want to press `Enter` on the selected Usage group so that I can inspect the Sessions behind an aggregate row.
2. As an Agent user, I want the detail view to show only Sessions belonging to the selected group so that unrelated data does not distract me.
3. As an Agent user, I want Sessions sorted from newest to oldest in detail so that recent activity is visible first.
4. As an Agent user, I want to see each Session's source-native Session id so that I can correlate the row with original local data.
5. As an Agent user, I want to see each Session's source so that mixed group detail remains interpretable.
6. As an Agent user, I want to see each Session's project so that I can tell where the Session ran.
7. As an Agent user, I want to see each Session's raw Model so that I can identify routing aliases without speculative resolution.
8. As an Agent user, I want to see each Session's start and end timestamps so that I can understand its duration.
9. As an Agent user, I want to see each Session's message count so that activity level is visible.
10. As an Agent user, I want to see input, output, cache-read, and cache-write tokens per Session so that the canonical Token Breakdown remains visible.
11. As an Agent user, I want output tokens to include reasoning tokens so that detail matches aggregate accounting.
12. As an Agent user, I want Cost shown for Sessions whose Source records it and absent otherwise so that missing spend data is not displayed as zero.
13. As an Agent user, I want detail to inherit the CLI filters already applied to `usage` so that the detail list matches the aggregate row.
14. As an Agent user, I want `Esc` in the detail view to return to the group table so that drill-down is non-destructive.
15. As an Agent user, I want the previous group selection to remain after returning so that I can quickly reopen the same detail.
16. As an Agent user, I want `q` in either view to quit the TUI so that the exit shortcut remains predictable.
17. As an Agent user, I want `j`/`k` and arrow keys in detail to move the selected Session so that navigation is consistent with the group table.
18. As an Agent user, I want `r` in detail to refresh data so that manual refresh behaves consistently.
19. As an Agent user, I want live refresh in detail to update the list so that newly written Sessions appear without manually returning to the group table.
20. As an Agent user, I want the selected detail row to remain sensible after a refresh when Session counts change so that the UI does not lose control unexpectedly.
21. As an Agent user, I want `Enter` to be a no-op when no group is selected so that an empty state does not trigger an error.
22. As an Agent user, I want the footer to advertise `Enter` in the group view and `Esc` in the detail view so that controls are discoverable.
23. As a power user, I want grouping by Project or Model to also support drill-down so that the interaction remains consistent across grouping dimensions.
24. As a power user, I want mixed-source group detail to retain each Session's own source-scoped Cost so that cross-Source cost math is never introduced.
25. As a power user, I want detail rows to use the shared K/M/B token formatter so that TUI readability is consistent.
26. As a power user, I want Claude Sessions with no cache data to distinguish unavailable cache values from a meaningful zero, consistent with the group view.
27. As a developer, I want the detail data selection to be modeled as pure TUI state so that it can be tested without driving a real terminal.
28. As a developer, I want one shared filtered Record set to feed both aggregation and detail so that aggregate totals and detail contents cannot drift.
29. As a developer, I want the background refresh path to deliver Records and Source statuses alongside the aggregate result so that all views stay synchronized.
30. As a developer, I want the detail implementation to reuse the existing Record, Filter, Source, and TUI rendering conventions rather than introducing a parallel data model.

## Implementation Decisions

- The detail view is an additional TUI state within the existing `usage` interactive mode; `--json` and `--csv` output remain unchanged.
- The shared filtered Record set is carried in TUI state. The aggregate result and detail list are both derived from that same Record set with the same grouping/filter semantics.
- The detail view is keyed by `(GroupBy, group key)`. The selected group key is captured when the user presses `Enter`, so background refresh cannot silently change the meaning of the open detail view.
- Detail Sessions are all filtered Records whose value for the current grouping dimension exactly equals the selected key. Cost is displayed per Session only; the detail view must not sum Cost across Sources.
- Detail Sessions are sorted by `started_at` descending. Ties preserve source order deterministically by sorting by source and Session id after timestamp ordering.
- The group table selection is separate from the detail table selection. Returning to groups preserves the original group selection.
- `Esc` in detail returns to groups. `Esc` in groups quits. `q` always quits.
- `Enter` is accepted only in groups mode. In groups mode with no result, no selection, or no matching Sessions, it is a no-op.
- `r` reloads sources, rebuilds the aggregate result, and rebuilds the open detail list if the selected group still exists. If the selected group disappears after refresh, the TUI returns to groups mode.
- Live refresh applies the same rules as manual refresh. The selected detail row is clamped to the refreshed list length.
- Cycling grouping dimensions closes detail because old detail keys are not meaningful under a different grouping dimension.
- The group table and detail table should render through stateful table rendering so selection changes scroll into view.
- The detail block title identifies the selected group key and Session count.
- Detail columns are: Session id, Source, Project, Model, Started, Ended, Messages, Input, Output, Cache R, Cache W, Cost.
- Timestamps are rendered in a compact local-readable UTC form; missing end time renders as `-`.
- Cost missing data renders as `-`; numeric cost keeps the existing source-scoped display behavior.
- Claude cache cells in the detail table use the same "No data" convention as the group table.
- Footer keys change by mode: groups mode advertises `Enter` for detail; detail mode advertises `Esc` for back.
- No CLI flags, config keys, JSON/CSV fields, data schema changes, or Source trait changes are required.
- No new dependency is required.

## Testing Decisions

- Good tests observe externally meaningful state transitions rather than terminal bytes. Because this feature is interactive-only, the highest available seam is the TUI state API: state mutation methods plus the rendered data they select.
- Test the state layer directly without creating a real terminal: construct a TUI state from an aggregate result and filtered Records, select a group, open detail, assert the detail Session list and view state, navigate, refresh, and close detail.
- Assertions must cover:
  - `Enter` with a selection opens detail for the selected group key.
  - Detail contains exactly the Records matching that group under the current grouping dimension.
  - Detail is sorted by `started_at` descending.
  - Detail respects mixed grouping keys by matching the current dimension's field, not forcing source equality.
  - `Esc` returns to groups while preserving the group selection.
  - Detail navigation moves selection within bounds and clamps correctly.
  - Refresh updates detail rows, clamps the selected detail row, and preserves the open group when it still exists.
  - Refresh closes detail when the selected group disappears.
  - Cycling grouping closes detail.
  - `Enter` is a no-op when no result, no selection, or no matching Sessions exist.
  - Cost remains per Session and is not summed in detail state.
- Keep existing integration tests unchanged because `--json` and `--csv` behavior is intentionally unchanged.
- Existing TUI rendering helpers continue to have focused unit tests for formatting and cache-cell display.

## Out of Scope

- Per-Session drill-down beyond the Session detail table, such as transcript contents or message-level tokens.
- Nested state where a detail Session can itself be opened into a deeper view.
- Any new `usage` CLI flag, config option, JSON field, or CSV column.
- Filtering inside the detail view; detail respects the filters already applied to the parent `usage` invocation.
- Sorting or grouping detail rows by any dimension other than `started_at` descending.
- Cross-Source cost totals in the detail header or rows.
- Changing how Sessions are listed by the standalone `sessions` command.
- Persistence of the open group across separate invocations.

## Further Notes

- With the default `--group-by source`, every group row is an Agent and this feature directly satisfies the requested Agent detail interaction.
- Supporting Project and Model drill-down is included because the selected row always has the same conceptual shape in the TUI; the implementation remains generic on the current grouping dimension.
- The Session list in detail is an aggregate-to-Session bridge, not a replacement for `sessions`; `sessions` remains available as a flat, scriptable command-level listing.
- Source-scoped Cost and raw Model conventions must remain unchanged.

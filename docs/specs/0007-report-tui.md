---
id: 0007
title: "report TUI: interactive report viewer by default"
status: ready-for-agent
created: 2026-09-06
triage: ready-for-agent
---

## Problem Statement

`llmhelper report` currently dumps a Markdown document to stdout when `--output` is absent. In a terminal this is a wall of pipe-delimited text that immediately scrolls away, offers no navigation, and is inconsistent with the other subcommands (`usage`, `diff`, `sessions`), which all present an interactive TUI by default. Users who just want to *read* the report have no good surface; users who want to *share* it already have `--output`.

## Solution

When `report` runs without `--output`, it enters an interactive TUI whose single purpose is to render the report: the exact same Markdown document produced by the existing pure renderer, presented as a scrollable, styled viewer. When `--output <path>` is given, the command keeps its current behavior — write the Markdown to the file, keep stdout empty — so scripted sharing is unaffected.

This supersedes spec 0006's decision that stdout is the default destination; from now on the interactive viewer is the default and `--output` is the file destination.

## User Stories

1. As a terminal user, I want `llmhelper report` to open a TUI by default, so that reading the report does not require scrolling back through stdout
2. As a terminal user, I want the TUI to render the same report content that `--output` writes, so that what I read on screen matches what I share
3. As a reader, I want to scroll through the report with ↑/↓ and j/k, so that I can move line by line through long tables
4. As a reader, I want PageUp/PageDown to scroll by pages, so that I can traverse a long report quickly
5. As a reader, I want `g`/Home to jump to the top and `G`/End to jump to the bottom, so that I can reach the totals or the sources appendix instantly
6. As a reader, I want scroll position clamped to the document bounds, so that I never scroll past the end into empty space
7. As a reader, I want a scroll-position indicator (visible line range and total line count) in the header, so that I always know where I am in the document
8. As a reader, I want Markdown headings and tables to be visually distinguished (color/weight), so that the report is scannable rather than a flat wall of text
9. As a reader, I want `q`/Esc to exit the viewer cleanly, restoring my terminal, so that the session ends predictably
10. As a script author, I want `--output <path>` to keep writing the Markdown file with stdout empty, so that existing automation keeps working unchanged
11. As a script author, I want file-write errors to keep failing with a clear message naming the destination, so that I can fix the path
12. As a developer, I want the report rendering to remain a pure function reused by both TUI and file output, so that the two surfaces cannot drift
13. As a developer, I want the TUI state (scroll offset, viewport height, quit flag) unit-testable without a terminal, following the existing state-level seam, so that behavior is verifiable in CI
14. As a developer, I want the TUI key handling and main loop to follow the same pattern as the other subcommand TUIs, so that the codebase stays uniform
15. As a user, I want all existing filter flags (`--last`, `--since`, `--project`, `--model`, `--source`, `--group-by`, `--top`, `--title`) to keep working in the TUI, so that the viewer honors exactly what was filtered
16. As a user, I want `--top` truncation and the `(+ k more …)` line visible in the TUI, so that the on-screen report reflects the invocation's parameters

## Implementation Decisions

- The report Markdown continues to be built once by the existing pure renderer (`ReportMeta` + `render_report`). The TUI is a *viewer over that string* — it does not re-aggregate, re-filter, or re-render the data model.
- Because the TUI renders a static document (not live-refreshing data), the report TUI does **not** spawn a background refresh task — a deliberate divergence from usage/diff/sessions TUIs; `r` refresh is not offered. Re-running the command regenerates the report.
- New TUI state module following the existing `SessionsTuiState` pattern: holds the report lines, a scroll offset, the last-known viewport height, and a running flag; exposes pure navigation methods (line up/down, page up/down, top, bottom) that clamp to document bounds; page size derives from the viewport height reported by the render loop.
- New render module: a header panel with the scroll indicator, a scrollable body rendering the styled Markdown lines, and a footer key-hint bar, reusing the shared dark theme palette and panel/footer helpers' conventions from the existing TUI render code.
- Markdown styling in the TUI: heading lines (leading `#`) render bold in the accent colors (level 1 primary accent, deeper headings the grouping accent), table rows render in text tone with their separator rows dimmed, bare horizontal rules dim, and the renderer's truncation note (`(+ k more …)`) mutes as meta-information. Styling is a pure function from a line to styled spans so it can be unit tested; the truncation-note shape is recognized through a shared helper exported by the report renderer rather than a duplicated magic string.
- Empty or blank documents show an explicit "empty" indicator placeholder instead of a misleading line range.
- Main entry: the `report` command builds the Markdown, then either writes it to the `--output` path (unchanged behavior, including the error message) or runs the report TUI.
- Terminal setup/teardown (raw mode, alternate screen, cursor restore) follows the shared pattern used by the other TUI entry structs.
- Key bindings: `↑`/`k` up one line, `↓`/`j` down one line, `PageUp`/`PageDown` half-page, `g`/`Home` top, `G`/`End` bottom, `q`/`Esc` quit. No other interactions.
- Markdown tables are rendered as aligned blocks rather than raw pipe rows: a header row immediately followed by a `|---|` delimiter row opens a table block; cells are padded to the per-column display width (CJK-aware via `unicode-width`), separated by ` │ `, with a dim `┼`-joined rule under the header. `\|` inside cells is unescaped; ragged rows pad missing cells; a lone pipe line without a delimiter row stays raw. Output keeps one line per input line so scroll math stays 1:1. This supersedes the earlier out-of-scope note on table alignment.

## Testing Decisions

- State-level unit tests on the new TUI state module (prior art: sessions/usage TUI state tests): navigation moves and clamps at both bounds, empty document is a no-op, page jumps use viewport height, top/bottom jumps, running-flag transitions. No terminal is required.
- Unit tests on the pure markdown-styling function: headings, table rows, rules, and plain text receive the intended styles (prior art: render helper tests).
- Integration tests via the CLI binary keep using the fixture-based harness:
  - All report *content* assertions now go through `--output <tempfile>` (previously they read stdout), since stdout is no longer produced without `--output`.
  - `--output` still writes the file and keeps stdout empty; unwritable paths still fail with the destination-naming error; validation errors (`--since`/`--last`, `--top 0`) still exit non-zero before any TUI starts.
  - No integration test exercises the TUI directly (it would block); interactive behavior is covered by the state unit tests plus a manual PTY smoke run at 80 and 130 columns.

## Out of Scope

- Any TUI interaction beyond scrolling and quitting (no filtering, re-grouping, or refresh inside the report viewer)
- Copy/export actions from within the TUI
- Markdown table column alignment/width negotiation beyond the raw text
- Writing other formats (JSON/CSV) from `report`
- Rendering a non-Markdown native widget layout of the report (the TUI shows the report document itself)

## Further Notes

- Spec 0006 story 2 ("stdout remains the default when `--output` is not given") is intentionally superseded by this spec; its `--output` file behavior stands.
- README's report section must be updated to describe the TUI default and the `--output` file path.
- AGENT_CHANGELOG gains an entry for this change.

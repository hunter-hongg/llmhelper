---
id: 0005
title: "report — shareable Markdown usage summary"
status: ready-for-agent
created: 2026-09-06
triage: ready-for-agent
---

## Problem Statement

`usage` answers aggregate questions interactively, `diff` compares two sliding windows, and `sessions` lists individual Sessions. But none of them produce an artifact a human can read later or share. When the user wants "a weekly summary I can paste into a chat, a PR, or a note", the only option today is `usage --json` plus external jq tooling, or a screenshot of the TUI.

The user needs a one-shot, non-interactive, human-readable rendering of the same filtered, aggregated Usage that `usage` already computes — a Markdown report that is useful pasted raw into Markdown-rendering surfaces (GitHub, chat tools, notes) and readable as plain text everywhere else.

## Solution

Add a fourth subcommand, `llmhelper report`, that loads all Sources through the existing registry, applies the same `Filter` as `usage`, aggregates once with the same `AggregateResult`, and renders a Markdown document to stdout.

The report has four sections:

1. **Header** — title, generation timestamp, the reporting window (from `--last`/`--since`, or "all time" when unbounded), and the applied non-temporal filters (project / model / source) when present.
2. **Totals** — grand Sessions, Messages, and the four-part Token Breakdown (input / output / cache-read / cache-write), plus Cost only as per-Source lines, never a cross-Source sum.
3. **Groups table** — one Markdown table for the `--group-by` dimension, sorted by total input tokens descending, with columns: key, sessions, messages, input, output, cache read, cache write, cost. Cost renders `—` when unavailable (mixed-source group or a Source that records no Cost) rather than `0`.
4. **Sources appendix** — one line per Source: name, loaded record count, and error status, so partial data is visible and never mistaken for absence.

`--top <n>` limits the groups table to the n largest groups by input tokens; the remaining groups collapse into a single `(+ k more …)` line. With no `--top`, every group is shown.

Output goes to stdout only; the user redirects to a file when they want to keep it. No file writing, no TUI, no network.

## User Stories

1. As an agent user, I want `llmhelper report --last 7d` so that I get a weekly usage summary in one command.
2. As an agent user, I want the report in Markdown so that I can paste it into a chat, a PR, or a note and it renders as a table.
3. As an agent user, I want the report readable as plain text so that it is still useful in a terminal that does not render Markdown.
4. As an agent user, I want a generation timestamp so that I know how fresh the numbers are.
5. As an agent user, I want the reporting window in the header so that I know which time range the numbers cover.
6. As an agent user, I want "all time" surfaced when no time filter is given so that an unbounded report is never mistaken for a recent window.
7. As an agent user, I want my project / model / source filters echoed in the header so that the report is self-describing when shared.
8. As an agent user, I want grand totals for sessions, messages, and the four token parts so that the report opens with the headline numbers.
9. As an agent user, I want Cost shown per Source and never summed across Sources so that spend numbers stay honest.
10. As an agent user, I want a Cost column in the groups table that renders an em-dash when Cost is unavailable so that missing spend data is never confused with free.
11. As an agent user, I want the groups table sorted by input tokens descending so that the biggest consumers are at the top.
12. As an agent user, I want `--top 5` to trim the table so that a large number of projects stays readable.
13. As an agent user, I want trimmed groups counted in a visible `(+ k more …)` line so that I know the table is truncated.
14. As an agent user, I want the Sources appendix so that I can see which Sources loaded, how many records each contributed, and which failed.
15. As an agent user, I want Source errors printed in the appendix so that missing data is explained.
16. As an agent user, I want an empty result to still produce a valid report with a visible "no sessions matched" note rather than an empty output.
17. As a script author, I want the report on stdout so that I can redirect, pipe, or archive it.
18. As a script author, I want deterministic output for identical inputs so that report diffs stay meaningful (timestamps excepted).
19. As a power user, I want the same filter flags as `usage` (`--last`, `--since`, `--project`, `--model`, `--source`) so that I do not learn a new filter vocabulary.
20. As a power user, I want `--group-by source|project|model` so that the report highlights the dimension I care about.
21. As a power user, I want `--last` and `--since` to remain mutually exclusive so that the window is unambiguous.
22. As a power user, I want raw Model values preserved (including the `auto` bucket) so that the report never invents resolved models.
23. As a multi-machine user, I want the same source-path override flags and config file support as `usage` so that `report` works wherever the data lives.
24. As a user, I want token counts formatted with the K/M/B magnitude formatter so that large numbers remain readable.
25. As a developer, I want the report renderer to be a pure function from `(AggregateResult, Filter, GroupBy, statuses, top)` to a string so that it is unit-testable without a terminal or a real source tree.
26. As a developer, I want `report` to reuse `Registry`, `Filter`, `AggregateResult`, and `format_tokens` so that no parallel data model or formatter is introduced.

## Implementation Decisions

- New `ReportArgs` in `cli.rs` mirroring the `usage` filter flags (`--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db`, `--since`, `--last`, `--project`, `--model`, `--source`, `--group-by`) plus `--top <n>`. No `--json`/`--csv` — Markdown is the single output format; scripting consumers already have `usage --json`/`--csv`.
- `--since` and `--last` are mutually exclusive, validated identically to `usage`.
- `--top 0` is invalid and errors; `--top` must be ≥ 1.
- A new `src/report.rs` module owns rendering: `render_report(&AggregateResult, &ReportMeta, &[SourceStatus], Option<usize>) -> String`. `ReportMeta` carries the window description, generation timestamp, grouping-dimension label, and the applied non-temporal filters. The function is pure — no I/O, no fallible paths — so it returns `String` directly and is testable with hand-built fixtures.
- `main.rs` gains `run_report(args)`: load config (merged with CLI overrides through the existing `Config::merge` path), discover sources, build the filter (same `Filter` construction as `usage`), aggregate once, render, print. Source-load errors are not fatal and surface in the appendix, matching the degrade-per-source convention.
- Markdown style:
  - `# llmhelper report` title, then a compact metadata list (generated, window, filters).
  - Totals as a single Markdown table row under a `## Totals` heading, with a separate `## Cost by source` table listing one row per Source that records Cost; Sources without Cost are omitted there, not shown as zero.
  - Groups table under `## Usage by <dimension>`, columns: `key`, `sessions`, `messages`, `input`, `output`, `cache read`, `cache write`, `cost`.
  - Token cells use `format_tokens` (K/M/B); session/message/cost cells are plain numbers. Cost keeps six decimals as elsewhere; unavailable cost renders `—`.
  - The per-Source cost table sums Cost only within a single Source (sum of that Source's groups), honoring source-scoped Cost.
  - Sources appendix under `## Sources`, one Markdown table: `source`, `records`, `status`.
- Group sorting is by `tokens.input` descending, tie-broken by key ascending for determinism. This differs from `usage --json`'s BTreeMap order deliberately: a report is for reading, not for joining. The cost-by-source table lists Sources in alphabetical order for the same reason.
- The trimmed `(+ k more …)` line reports how many groups were omitted and their combined input tokens.
- Empty result: totals render zeros, the groups table renders a `no sessions matched` line instead of rows, and the appendix still renders.
- No new dependencies. No TUI. No file writes. The command stays read-only.

## Testing Decisions

- Unit tests target `render_report` directly with hand-built `AggregateResult`/`SourceStatus` values — the pure-function seam from Implementation Decisions. Assertions:
  - Header contains title, generation timestamp, window description ("last 7d", "since …", "all time") and echoes filters.
  - Totals row reflects grand_sessions/grand_messages/token sums.
  - Cost appears per Source in the cost-by-source table and never as a cross-Source total.
  - Groups table sorts by input descending.
  - `--top` truncates to n rows plus an accurate `(+ k more …)` line with summed input of omitted groups.
  - Unavailable cost renders `—`, not `0`.
  - Empty aggregate renders the no-match note and stays valid Markdown.
  - Source errors appear in the appendix with their status text.
- Integration tests run the built binary against the existing `tests/fixtures` tree, asserting: exit success, Markdown structure (headings, table pipes), filter echo, and that a `--top 1` invocation produces the `(+ k more …)` line.
- Existing tests must pass unchanged; no behavior of `usage`/`diff`/`sessions` may change.

## Out of Scope

- Any output format other than Markdown (no `--html`, no `--json`).
- Writing files, sending reports anywhere, or scheduling (no cron, no daemon).
- A TUI or interactive mode for `report`.
- Diff-style window comparison inside the report; that is `diff`'s job.
- Per-Session listings inside the report; that is `sessions`' job.
- Resolving model routing aliases or cross-Source cost totals (permanent project conventions).
- Calendar-aligned windows; `--since` and `--last` only.

## Further Notes

- The report intentionally reuses the entire existing pipeline: adding it required a new `Command` variant and a pure renderer, no pipeline changes — which is the architecture working as intended.
- `report` is the first non-TUI default command; the project convention of "TUI unless --json/--csv" does not apply here because the report's consumer is usually a pipe or a file, not a human at a keyboard. A human at a keyboard already has `usage`.

---
id: 0015
title: "budget — source-scoped spend thresholds with report and TUI annotation"
status: done
created: 2026-09-13
triage: done
---

## Problem Statement

`llmhelper` can tell you what you *did* spend (per-Source cost in `report`,
`usage`, and `export`), but nothing tells you when that spend has crossed a
line you drew in advance. A user who wants to keep OpenCode under $5/day has to
read the number, remember the threshold, and compare it by eye — every time,
for every Source.

The project already has a place to put such a rule: `~/.config/llmhelper/config.toml`.
It configures Source paths, the UI refresh interval, and the `[request]`
defaults, but there is no vocabulary for "how much is too much".

The hard constraint is the Cost invariant recorded in `CONTEXT.md` and enforced
structurally in `aggregator.rs`: **Cost is always source-scoped — it must never
be summed or averaged across Sources.** Claude Code records no spend at all, so
its Cost is permanently absent. A naive "total spend" budget would have to sum
across Sources and would therefore be wrong by construction, and would silently
treat Claude Code's absent cost as zero. A budget feature must respect this
invariant rather than paper over it.

## Solution

Add a source-scoped spend budget: a named rule that, for one Source over one
time window, names a ceiling on Cost. Budgets are declared in `config.toml`
under a `[budget.<name>]` table and may also be supplied or overridden on the
command line. When `usage` or `report` runs, each budget is evaluated against
the filtered Records and its result is surfaced as annotation — an over-budget
Source is highlighted in the TUI and called out in a `## Budget` section of the
Markdown report.

A budget is **always bound to exactly one Source**. There is no cross-Source
budget, because there is no correct number to compare such a budget against.
A budget whose Source records no Cost (Claude Code) is reported as
`not measured` rather than silently passing or failing — the command must not
imply that unspent-unknown is the same as under-budget.

```
# ~/.config/llmhelper/config.toml
[budget.opencode-daily]
source = "opencode"
window = "1d"
max_cost = 5.00

[budget.omp-monthly]
source = "omp"
window = "30d"
max_cost = 40.00
```

```bash
# evaluate every configured budget alongside the normal report
llmhelper report --last 7d

# supply a one-off budget without touching the config file
llmhelper usage --budget opencode:5.00 --budget-window 7d

# reference a configured budget by name
llmhelper report --budget-name opencode-daily
```

Cost annotation is opt-in only in the sense that it does nothing when no budget
is configured: a run with no `[budget.*]` table and no `--budget` flag is
byte-for-byte unchanged from today.

## User Stories

1. As a user on an API plan, I want to declare a daily spend ceiling for one Source so that I can see at a glance whether today's OpenCode usage has crossed it.
2. As a user, I want the budget window to be independently selectable (daily, 7d, 30d, or any `--last`-style duration) so that I can set a daily ceiling and a monthly ceiling against the same Source.
3. As a user, I want budgets to be scoped to exactly one Source so that the number is always meaningful and never a cross-Source sum the project forbids.
4. As a user, I want a budget whose Source records no Cost (Claude Code) to be reported as `not measured` so that the command never implies absent data is zero spend.
5. As a user, I want an over-budget Source highlighted in the `usage` TUI so that I notice it without reading every cell.
6. As a user, I want a `## Budget` section in the Markdown report so that a shared report carries the threshold status with it.
7. As a user, I want to supply a one-off budget on the command line (`--budget source:amount`) so that I can check a hypothesis without editing my config.
8. As a user, I want to name a configured budget on the command line (`--budget-name <name>`) so that I can evaluate exactly one rule instead of all of them.
9. As a user, I want the report and TUI to keep working exactly as before when no budget is configured so that adding this feature does not disturb existing workflows.
10. As a user, I want a malformed budget config (unknown Source, non-positive amount, unparseable window) to fail loudly with a message naming the offending budget so that a typo does not silently disable my ceiling.
11. As a user, I want multiple budgets for the same Source with different windows to be evaluated independently so that a daily and a monthly ceiling can coexist.
12. As a user, I want the budget annotation to state the Source's actual spend and the ceiling next to each other so that I can see how far over (or under) I am.
13. As a script author, I want budget status to appear in the report Markdown (not only in the TUI) so that a pipeline that captures `report --output` also captures threshold violations.

## Implementation Decisions

### Domain

- New `src/budget.rs` holds the budget domain and its evaluation, kept independent of CLI and rendering:
  - `Budget { name: String, source: String, window: BudgetWindow, max_cost: f64 }`.
  - `BudgetWindow` is either `Calendar { days, label }` (a trailing N-local-day window anchored at local midnight today: `1d` is today, `1w` the last 7 local days, `1mo` the last 30 local days) or `Duration(chrono::Duration, label)` (evaluated as `now - window ..= now`, reusing the existing `parse_duration` semantics of `--last`). A third variant, `Unparsed(String)`, lets the config layer carry a bad window through load as an infallible value; validation then rejects it with the budget named.
  - `BudgetStatus { budget: Budget, spend: Option<f64>, state: BudgetState }` where `BudgetState` is `Under`, `Over`, or `NotMeasured`.
  - `fn evaluate(budgets: &[Budget], records: &[Record], now: DateTime<Utc>, command_since: Option<DateTime<Utc>>) -> Vec<BudgetStatus>` — a **pure function**. It filters `records` to the budget's `source` and window, then:
    - if the Source contributes no Records *or* none of its Records carry a Cost, `state = NotMeasured`;
    - otherwise `spend = Some(sum of that Source's per-Record cost)` and `state` is `Over` when `spend > max_cost`, else `Under`.
  - `fn evaluate_with_measurement(...) -> Vec<EvaluatedBudget>` wraps the above and attaches a `Measurement { lower_bound, clipped_by }`, so the renderer can tell whether a budget could see its whole window or was clipped by the command's own `--last`/`--since`. `command_since` is computed once per run and shared by every budget in it.
- Evaluation reuses the existing `Filter` window semantics so a budget window and a `--last` window are computed identically. The budget's own window is **independent** of the command's `--last`/`--since`: a 30d budget evaluated by a `--last 7d` report still looks at 30 days. (See Implementation Decision "Window independence" below.)
- `CalendarWindow` anchors on the *local* midnight, so a "daily" budget means the user's day, not UTC's. `1w` and `1mo` are trailing local-day windows (`7` and `30` days) anchored at today's local midnight, not ISO weeks or calendar months — the label echoes the spelling the user wrote, and the README states the trailing-window meaning. The timestamp used for bucketing is `Record.started_at` (the same field every other window predicate uses).

### Configuration

- Extend `ConfigTable` in `src/config.rs` with `budget: Option<BTreeMap<String, BudgetConfig>>` (a `[budget.<name>]` table). `BudgetConfig` deserializes `source: String`, `window: String`, `max_cost: f64`.
- `Config` gains `budgets: Vec<Budget>` (empty by default), preserving the file's map order as a stable, sorted-by-name order for deterministic output.
- Each `Budget.name` is its table key. This is required for `--budget-name` and so that error messages can name the offending budget.
- Validation happens **after** load and at the CLI boundary, via `budget::validate` / `budget::validate_all`. `Config::load_with` keeps its current infallible contract and stores any bad window in `BudgetWindow::Unparsed`; the command entry points (`usage`, `report`) call `BudgetArgs::resolve_budgets`, which validates every budget before any records are loaded. Rules:
  - `source` must be one of the four known Source names (`claude`, `opencode`, `omp`, `kilo`); anything else errors naming the budget and the bad Source.
  - `max_cost` must be finite and `> 0.0`; zero or negative errors.
  - `window` must parse as a duration via the existing `parse_duration`, or match a calendar keyword (`1d`, `1w`, `1mo`); otherwise errors naming the budget.
  - A malformed budget is a command failure (exit 1), not a warning. A ceiling that silently disappears is worse than a command that refuses to run.

### CLI

- New flags on `UsageArgs` and `ReportArgs` (and only those two — see Out of Scope):
  - `--budget <source:amount>` (repeatable) — a one-off, duration-window budget. The window comes from `--budget-window` (default `1d`). Bad syntax errors naming the flag.
  - `--budget-window <duration>` — the window applied to `--budget` one-offs. Ignored when no `--budget` is given.
  - `--budget-name <name>` (repeatable) — restrict evaluation to the named configured budget(s). An unknown name errors listing the configured names.
- Precedence: `--budget-name` selects a subset of configured budgets; `--budget` adds anonymous one-offs. Both may be used together. With neither, every configured budget is evaluated (or none, if none are configured).
- Because `UsageArgs` and `ReportArgs` share filter/source-path plumbing via the `FilterArgs`/`SourcePathArgs` traits, the budget flags are **not** added to those traits — budget is not a record predicate, it is a presentation/annotation concern. The flags live on the two structs directly, sharing behavior through a new `BudgetArgs` trait whose `resolve_budgets` is the single place the three flags combine.

### Rendering

- **Markdown report** (`render_report`, `src/report.rs`): gains a `budgets: &[EvaluatedBudget]` parameter. When non-empty, appends a `## Budget` section after `## Cost by source` and before `## Usage by <dimension>`:
  ```
  ## Budget

  | budget | source | window | spend | max | status |
  |---|---|---|---|---|---|
  | daily-opencode | opencode | 1d | 6.120000 | 5.000000 | over |
  | monthly-omp | omp | 30d | 12.400000 | 40.000000 | ok |
  | cc-guard | claude | 7d | — | 5.000000 | not measured |
  ```
  Spend uses the same `{:.6}` cost formatting as the existing Cost table; `—` when `NotMeasured`. Status is the lowercase word `over` / `ok` / `not measured`. When no budgets are configured the section is omitted entirely (no empty header).
- **`usage` TUI**: a Source row whose Source has an `Over` budget is highlighted with the existing warning style — the group key is prefixed with a `⚠` marker colored with the existing `RED` palette role, occupying the same two-character gutter as the plain selection padding, so no column is added and the 80/130-column layouts are undisturbed. The header's `grouped by` line gains a short budget indicator (`budget: 1 over` / `budget: ok` / `budget: not measured`) only when at least one budget exists, rendered inline so the header height is unchanged. Full budget detail (spend vs. max) lives in the report, not the TUI.
- `Cost` remains `—` and never coerced to `0.0` for Sources that do not record it; the budget feature must not change that.

### Window independence

A budget's window is its own property and is deliberately **not** intersected with the command's `--last`/`--since`. Rationale: a `1d` budget is meaningful only against "today", regardless of whether the report was asked for 7 days. However, a budget can only be evaluated over Records the command actually loaded, so the effective window is the budget window intersected with what the Source returned. This is documented behavior, not an accident: a 30d budget in a `--last 7d` report is measured over 7d of data and the report must say so.

Implementation: the Budget section's `window` cell shows the budget's declared window; when the command's filter is narrower than a given budget, that budget's row is followed by a derived note naming the budget, the narrower bound, and the declared window (e.g. `monthly: measured over the loaded range only (from 2026-09-06T00:00:00Z); its 30d window reaches further back…`). A budget whose window fits inside the loaded range gets no caveat. Each budget carries its own `Measurement.clipped_by`, so the caveat is per-budget and reflects the values actually in effect.

## Testing Decisions

- **Pure-function tests in `src/budget.rs`** (the bulk): `evaluate` over constructed Records — under, exactly-at (boundary: `spend == max_cost` is `Under`, since the rule is "crossed", i.e. strictly greater), over, no-records-at-all, records-without-cost (Claude), and multi-Source input where only the budget's Source contributes.
- **Window tests**: a `Duration` window includes/excludes on the boundary; a `Calendar` window resets at local midnight (construct timestamps around the boundary; test with an explicit `now` so the test is not wall-clock flaky — follow the relative-timestamp discipline the `diff` tests already use).
- **Config tests** in `src/config.rs`: a `[budget.*]` table parses into ordered `Budget`s; unknown Source, non-positive `max_cost`, and bad `window` each error naming the budget; absent `[budget]` yields an empty vec.
- **Report tests** in `src/report.rs`: the `## Budget` section renders the three statuses correctly, is omitted when empty, and the `not measured` row shows `—` not `0.000000`.
- **CLI/integration tests** in `tests/integration.rs`: `--budget opencode:5.00 --budget-window 1d` produces an over/ok status against a fixture; `--budget-name` selects one configured budget; an unknown `--budget-name` errors listing configured names; a bad `--budget` syntax errors; `report --output` carries the `## Budget` section.
- **Regression guard**: a run with no budgets configured must render output byte-identical to today. Add an assertion that `render_report` with `budgets: &[]` equals the pre-existing output for a fixed fixture (the existing report tests already pin the exact string, so this is mostly ensured by making `budgets` default-empty — but state it explicitly as a test).
- No new dependencies. `chrono` already provides calendar arithmetic and `toml`/`serde` already provide the config deserialization.

## Out of Scope

- **Cross-Source budgets.** Forbidden by the Cost invariant; there is no correct number to compare. A future "total spend" feature would require resolved per-Source pricing, which this project deliberately does not do.
- **Notifications, alerts, exit codes, or CI gating.** The chosen behavior is annotation only: highlight in the TUI, a section in the report. No nonzero exit on over-budget, no webhooks, no email.
- **Editing budgets from inside the TUI.** Budgets come from config or flags; the TUI only displays their evaluation.
- **Budgets on `export`.** `export` is the machine-oriented dump and prints no headers or prose (by its own spec). Budget annotation would pollute a JSONL consumer. Out of scope for `export`, and likewise for `diff`, `sessions`, and `search`.
- **Token-based budgets.** The user chose Cost budgets for this spec. A token budget is a different measurement and is not included here.
- **Historical budget tracking / burn-rate / projection.** This spec evaluates a budget against the current data; it does not store history, forecast when a budget will be exhausted, or alert on rate of spend.
- **Per-project or per-model budget scoping.** A budget binds to exactly one Source in this spec; finer scoping is a possible follow-up but multiplies the "what does the number mean" question.

## Further Notes

- This is the first feature to add a new top-level `config.toml` section (`[budget]`) since `[request]`. The config loader is currently a flat struct-of-Options; a `BTreeMap<String, BudgetConfig>` is the first keyed table, which is why `Budget.name` must come from the key rather than a field.
- The `## Budget` section is placed after `## Cost by source` because it is a derived view of exactly that data — a reader sees the raw per-Source spend first, then the threshold judgment on it.
- The `over` boundary is deliberately **strictly greater than** the ceiling: `spend == max_cost` is `ok`. A ceiling of $5.00 is not crossed at exactly $5.00. This must be stated in the report legend or the status word to avoid ambiguity.
- Claude Code's permanent absence of Cost is the single most likely source of user confusion here. The `not measured` status exists specifically so that a user who writes `[budget.x] source = "claude"` is told the truth rather than shown a misleading `ok`.

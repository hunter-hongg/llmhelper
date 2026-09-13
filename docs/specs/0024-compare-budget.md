---
id: 0024
title: "compare — mark which ranked groups are over budget"
status: done
created: 2026-09-16
updated: 2026-09-16
triage: done
---
## Problem Statement
`compare` (spec 0023) ranks the groups of one window by a chosen metric and
shows each one's share, answering *who is the big spender*. `budget`
(spec 0015) answers the complementary question *am I over my red line* — but it
evaluates **per Source**, and only surfaces in `usage`/`watch` (a `⚠` on the
source row and a header indicator) and in `report` (a `## Budget` section).
Neither meets in `compare`. A user staring at a ranked `--group-by project`
table — "who is eating my budget?" — has to cross-reference the project rows
against a separate `usage` run to learn whether the top spender is *the one
that just blew the ceiling*. The two facts live in two commands with two
formats, and the reader does the join in their head.
The gap is precise: `compare` shows **magnitude** but not **status**. A project
with 4.2M tokens and a project with 4.1M tokens render as adjacent rows even
when one belongs to a Source that is \$2 over its budget and the other does
not. The tool already computed the budget verdict (in the same process, from
the same records); it just does not put it on the row the reader is looking at.
## Solution
Add budget awareness to `compare` as a **pure annotation**, exactly as
spec 0015 established for `usage`/`report`: budgets are declared and evaluated
by the existing machinery, and each ranked row that maps to an over-budget
Source is marked. No new budget semantics, no change to ordering, shares, or
exit code.
```bash
# One-off budget, marked inline on the ranked rows
llmhelper compare --last 7d --group-by source --budget "opencode:5/1d"

# Configured budgets from config.toml, ranked by cost, over rows flagged
llmhelper compare --last 30d --group-by source --sort-by cost

# JSON consumers get the status as a field, not a glyph
llmhelper compare --last 7d --group-by source --json
```
Terminal output marks the over row on the same side the table already uses for
`rank`, so no column is added:
```
sources: claude ●  opencode ●  omp ●  kilo ●
compare  |  group: source   sort: tokens   rows: 4

rank  group     sessions  messages   tokens   share      cost
 ⚠ 1  opencode         8        41    4.2M    61.3%  6.200000
   2  omp              3        12    1.8M    26.1%  1.100000
   3  kilo             2         7    0.6M     8.9%  0.400000
   4  claude           1         3    0.3M     3.7%        —
```
### The Source-scope mapping (the load-bearing decision)
A `Budget` is scoped to **exactly one Source** that must record Cost; this is
not decoration, it is the only correct scope (spec 0015: `aggregator` sets a
group's `cost` to `None` the moment it mixes sources, and Claude Code records
no cost at all). A `compare` row carries the same constraint through its
group's `source`: `"mixed"` when the group spans sources, otherwise the single
source name.

**This spec had to first make `Group.source` truthful.** The aggregator's
mixing rule was buggy: it only marked a group `mixed` when the *second* source
appeared in a branch where the first source had no cost yet, or where the
newcomer had none. When both Sources reported Cost, the "both Some" branch
summed them **without comparing sources**, so a group that genuinely spanned
two Sources kept its first Source's name. The fixtures show it: `/repos/test`
is kilo+opencode (cost 1.5 + 0.17) yet aggregated as `source = "kilo"`, cost
`1.67`. A Source-scoped budget would then mark that mixed project `⚠` — exactly
the lie this spec forbids. The fix separates mixing from summing: a group is
`mixed` (and permanently cost-less) the moment a differing Source contributes,
independent of Cost. This is a **shared-model correctness fix** — it also stops
`usage` from naming a single Source for a multi-Source group — and is
regression-tested in `src/aggregator.rs`.
Therefore a row is marked **over** iff its group's `source` matches a budget
whose evaluated state is `Over`. A `"mixed"` row is **never** marked — it has
no single source to attribute the verdict to, and a budget's spend is the
Source's spend, not a project's. This is the same rule the TUI already applies
in `source_is_over_budget`; `compare` reuses it rather than inventing a
second mapping.
Three honest cases fall out, and all three render:
- **Over** → row marked (`⚠` / `"over"` / `over`).
- **A budget exists for the row's Source but is `Under`** → row carries no
  mark. Absence of `⚠` means "measured and fine", because the budgets are
  known to the renderer.
- **A budget exists but the Source is `NotMeasured`** (no cost recorded) → the
  row carries no mark and no claim of `ok`. A `—` cost cell already says the
  cost is unknown; stamping `under` would be the lie spec 0015 refused.
- **No budgets configured** → the *budget* output is entirely absent: the mark,
  the header count, and the JSON field are absent, not empty, so every existing
  consumer of the budget machinery is untouched. Note this guarantee is about
  the budget layer only, **not** about the whole command: the aggregator fix
  below is a deliberate behavior change that is visible with no budgets at all.
  A `--group-by project`/`model` run whose group spans Sources now reports
  `source = "mixed"` and `cost = —`/`null` where it previously named the first
  Source and printed its summed cost. That change is the *point* of the fix —
  the old output was the lie that could misattribute a Source-scoped budget —
  and it is therefore correct that it is not byte-identical. Byte-identity holds
  only for groupings that cannot produce a mixed row (`--group-by source`), and
  the guard test covers exactly that case; it is not a claim about the other
  dimensions.
### Wire-up (reuse, don't rebuild)
- `CompareArgs` gains the three existing `BudgetArgs` accessors (`--budget`,
  `--budget-window`, `--budget-name`) by implementing the shared `BudgetArgs`
  trait — the same trait `usage`/`report`/`watch` already implement, so the
  flag surface and validation are identical by construction.
- `run_compare` resolves budgets with `args.resolve_budgets(&config.budgets)`
  **before any source is read** (spec 0015's ordering), then evaluates them
  with `budget::evaluate_with_measurement(&budgets, &records, now,
  command_since)` against the same filtered record set and the same single
  `now` the window was anchored with — so a budget and the ranking cannot
  disagree about what "this window" is.
- The over-Source set is derived once by a pure function in `compare.rs`,
  `budget_states(&[EvaluatedBudget]) -> BTreeMap<String, BudgetState>`; the
  renderers read each row's already-attached state and never re-derive it, so
  there is a single source of truth for "is this row over".
- `RankedGroup` gains `budget_state: Option<BudgetState>` — `None` when no
  budget bears on the row (including `mixed`), otherwise the state of the
  Source's budget. **But** because `rank()` is pure and budget-free (spec 0023's
  contract), the state is attached **after** ranking, by
  `attach_budget_states(rows, sources, states)`, which needs the `Group::source`
  of each row. `rank_with_sources` returns that parallel `Vec<Option<String>>`
  alongside the rows (the `(others)` slot is `None`), so `rank()` itself keeps
  its signature and contract and every existing caller is untouched.
### Rendering
- **Table**: a two-character status gutter, left of `rank`, holding `⚠ ` for an
  over row and two spaces otherwise — mirroring the TUI's fixed-width gutter so
  80/130-column layouts do not shift. When no budgets are configured the gutter
  is **omitted entirely**, keeping the no-budget output byte-identical.
  The header line appends `   budgets: N over` (or `   budgets: ok`, or
  `   budgets: not measured`) when budgets exist, reusing the wording
  `budget_indicator` already fixed.
- **JSON**: each row gains `budget_state` (`"over"` / `"under"` /
  `"not_measured"`, via a new `BudgetState::json_label`) with
  `skip_serializing_if = "Option::is_none"`, and the payload gains a `budgets`
  object only when budgets exist. The slug vocabulary is deliberately distinct
  from `report`'s prose label (`over`/`ok`/`not measured`): a machine consumer
  gets a whitespace-free token while the human table keeps its sentence-like
  wording. `under` is the slug for the `Under` variant because `ok` is a
  rendering choice, not the variant's name.
- **CSV**: a `budget_state` column is appended **only when budgets exist**;
  without budgets the header is exactly today's 15 columns. An absent state is
  an empty cell, never `under` or `0`.
Exit code stays `0` regardless of budget state — a budget is an annotation,
not a gate (spec 0015).
## User Stories
- As a user ranking projects by tokens, I see a `⚠` on the row whose Source is
  over its budget, so I can connect "biggest" with "over the line" in one view.
- As a JSON consumer, I read `budget_state == "over"` per row instead of parsing
  a glyph, and I know a missing field means "no budget bears on this row".
- As an existing `compare` user with no budgets, my output, tests, and
  downstream parsers are unchanged byte-for-byte.
- As a user grouping by project across sources, I understand that a
  cross-source mixed row is never marked, because a budget is a Source's spend,
  not a project's.
## Implementation Decisions
- The **only** new domain logic is the Source→state map and the post-rank state
  attachment; everything else is wiring into existing `BudgetArgs` /
  `resolve_budgets` / `evaluate_with_measurement`.
- The aggregator's mixing rule is corrected so `Group.source` is `"mixed"` for
  any multi-Source group, regardless of whether both Sources report Cost. This
  is a prerequisite for the annotation to be truthful, not an optional cleanup.
- `rank()` keeps its signature (minus the injected state). The state is a
  property of the *row's group source*, resolved against the evaluated budgets,
  so it is applied in `run_compare` after `rank` returns.
- Grouping by `source` and grouping by `project`/`model` are handled by the
  same rule: match the group's `source` field, which is the single source or
  `"mixed"`. No per-dimension special case.
- Budgets are resolved and validated **before** the disk scan, so a bad
  `--budget` fails with the same message `usage` gives and never triggers a
  load.
## Testing Decisions
- **`budget_states` is unit-tested in `src/compare.rs`**: every budget maps its
  Source to its state; a Source with no budget is absent from the map.
- **`attach_budget_states` is unit-tested**: a `source`-grouped row matches its
  budget; a `mixed` row gets `None`; the `(others)` row gets `None`; the
  `sources` vector stays parallel to the rows; `rank()` alone leaves every row
  `None`.
- **The CLI seam is integration-tested in `tests/compare.rs`**:
  - With `--budget "opencode:0.10/1d"` and an over-spending opencode fixture, the
    `opencode` row is marked and the others are not.
  - A `--group-by project` budget on a Source that contributes to a *mixed*
    project marks no row (regression for the aggregator fix).
  - With no budgets, a `--group-by source` run (a dimension that cannot produce
    a mixed row) is byte-identical to a run before this spec's budget layer:
    no glyph, no `budgets:` header, no `budget_state` field (snapshot guard,
    like spec 0015's report guard). The guard deliberately does **not** claim
    byte-identity for `project`/`model`, where the aggregator fix changes the
    output on purpose; a separate test asserts the mixed row's `mixed`/`None`
    shape instead.
  - `--json` rows carry `budget_state` only when budgets exist, and the value
    is `"over"` for the offending Source.
  - `--csv` appends the column only with budgets; without budgets the header is
    the original 15 columns.
  - Exit code is `0` with an over budget.
  - `--budget-name unknown` errors before loading, with `usage`'s message.
- **Source-path isolation**: every test passes all four override flags; empty
  cases point them at nonexistent temp paths (the trap specs 0018/0019/0022/0023
  all recorded).
## Out of Scope
- **Changing ranking, shares, or the `(others)` fold.** A budget never
  reorders rows or alters a share; it annotates. The `(others)` row is never
  marked (it is synthetic, not a Source).
- **Per-project budgets.** A budget is Source-scoped by data-model necessity
  (ADR 0001). A project's cost can be `None` when it mixes sources, so a
  project budget would be unmeasurable. Not added here.
- **Exit codes / CI gating on budget state.** A budget is advisory (spec 0015);
  `compare` does not become a gate.
- **TUI / live view for `compare`.** `compare` ships table/JSON/CSV only
  (spec 0023); budget marking in `usage`/`watch` is spec 0015/0019.
- **New budget semantics** (windows, sources, cost rules). This spec consumes
  spec 0015's model verbatim.
## Further Notes
- The tempting shortcut — sum a project's cost and compare it to a project
  budget — is exactly the mistake `aggregator` structurally prevents by
  nulling `cost` on a mixed group. Marking from the Source verdict keeps the
  annotation truthful even when the grouping dimension is finer than the budget
  scope.
- The `⚠`-in-a-fixed-gutter choice is deliberate: it reuses the TUI's
  precedent so the two views of "over budget" look alike, and it adds no column
  width, so existing `compare` column-layout tests and terminal-width
  assumptions hold.

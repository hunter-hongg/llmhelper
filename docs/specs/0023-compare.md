---
id: 0023
title: "compare — rank groups against each other within one window"
status: done
created: 2026-09-15
updated: 2026-09-15
triage: done
---
## Problem Statement

Every read command in this tool summarizes *one* thing at a time. `usage` groups
records by source/project/model and prints one row per group — but in an
arbitrary, insertion-derived order, with no measure of how much of the total
each group represents. `diff` contrasts the *same* group across *two* time
windows. `trend` walks the *same* group across a *sequence* of buckets.

None of them answers the question a user asks when staring at a month of usage:
*"who is eating my budget?"* A `usage --group-by project` table with 14 rows
forces the reader to do the ranking and the share arithmetic in their head —
add up the tokens, find the biggest row, estimate it is "maybe half". The
tool already has every number required to answer this; it just prints them
unsorted and unqualified.

The gap is precise: there is no command that **orders groups by a chosen
metric and reports each one's share of the whole**. That is not `usage`
(sorted? no), not `diff` (two windows), not `trend` (time axis). It is a
rank of the groups *against each other* inside a single window.

## Solution

Add a subcommand, `llmhelper compare`, that aggregates the filtered records by
one dimension and prints the resulting groups **ordered by a chosen metric,
descending**, each with its **share of the grand total**.

```bash
llmhelper compare --last 30d --group-by project
llmhelper compare --last 7d --group-by model --sort-by cost
llmhelper compare --last 30d --group-by source --top 5 --json
```

The output is a ranked table, largest first:

```
rank  project                         sessions  messages    tokens   share   cost
   1  /home/hunter/experimental/a          42       318    4.2M    61.3%   1.820000
   2  /home/hunter/experimental/b          18       120    1.5M    21.9%   0.610000
   3  /home/hunter/experimental/c           9        44  980.1K    14.1%        —
   ...
```

`compare` reuses the pieces that already exist rather than introducing a second
data path:

- The **records** come from the same `Registry::load_all` every read command
  uses.
- The **filter** is the same `Filter` (window → project → model → source), built
  from the same `FilterArgs` accessors, so `--explain` and the funnel behave
  identically.
- The **grouping** is the same `GroupBy` dimension and the same
  `AggregateResult::from_filtered_refs` that `usage` uses — so sessions,
  messages, tokens, and the source-scoped cost rule cannot drift between the two
  commands.
- The **diagnostics** are the same `diagnostics::diagnose` funnel spec 0020
  defined.

What `compare` adds on top:

1. **A ranking.** Groups are ordered by a single, explicit sort key, descending,
   with a stable tiebreak (see below). The order is the answer, not a side
   effect of hash or insertion order.
2. **A share column.** Each group's tokens as a percentage of the grand total,
   plus per-token-field shares, so "how big is this, really" is one number
   instead of a mental division.
3. **Top-N with an `others` fold.** `--top N` collapses everything past rank N
   into a single `(others)` row that still carries the remaining share, so a
   wide histogram becomes a readable leaderboard without silently dropping the
   tail.

## User Stories

1. As a user, I want `llmhelper compare --last 30d --group-by project` to list
   my projects largest-first, so that I can see which one dominates without
   sorting the table myself.
2. As a user, I want each group's tokens shown as a percentage of the total, so
   that I can say "this project is 60% of my usage" as a number I did not have
   to compute.
3. As a user, I want `--sort-by tokens|cost|sessions|messages` so that I can rank
   by the metric that matters to my question, defaulting to tokens.
4. As a user, I want the share percentage to be computed from the *same* metric
   I sorted by where that is meaningful (cost sorts by cost and reports cost
   share), so that the ordering and the share agree.
5. As a user, I want per-field token shares (input/output/cache_read/cache_write)
   available, so that I can see whether a dominant project is dominant in cache
   reads or in fresh input.
6. As a user, I want `--top N` to show only the N largest groups plus an
   `(others)` row, so that a long tail does not bury the ranking.
7. As a user, I want the `(others)` row to carry the *sum* of the folded groups'
   values and shares, so that the displayed shares still account for 100%.
8. As a user, I want `--group-by source|project|model` to choose the dimension,
   so that I can rank sources, projects, or models.
9. As a user, I want `compare` to require `--last` (or accept the shared window
   flags), so that a bare `compare` gives an actionable error rather than a rank
   of an undefined span.
10. As a user, I want `--project`, `--model`, `--source` to narrow the input
    *before* ranking, so that I can rank the projects inside one source.
11. As a user, I want `compare --json` to emit a structured ranked array, so
    that a script can consume the leaderboard without parsing the table.
12. As a user, I want `compare --csv` to emit a bare header plus one line per
    ranked group, so that the output drops into a spreadsheet.
13. As a user, I want `--explain` to report the same funnel every other read
    command reports, so that "why is this compare empty" has one answer.
14. As a user, I want an empty result to still be a valid, empty-ranked output
    (exit 0), so that "nothing matched" is not an error.
15. As a user, I want cost to be reported per group only when the group is
    single-source and every contributing record reports cost, so that ADR 0001's
    "never sum cost across sources" invariant holds — and I want this to be the
    *same* rule `usage` applies, not a second, weaker one.
16. As a user, I want `--json` and `--csv` to be mutually exclusive, so that I
    cannot ask for two encodings at once.
17. As a user, I want ties broken deterministically, so that two runs on the same
    data produce byte-identical output and I can diff them.
18. As a user, I want `--top 0` or a `--top` larger than the group count to
    behave sanely (no `(others)` row when nothing is folded), so that the flag
    has no surprise edge behavior.
19. As a user, I want a Source that fails to load to degrade exactly as it does
    in `usage` (a `SourceStatus` error, zero records for that source), so that
    one broken source does not fail the whole compare.
20. As a user, I want `compare` to change nothing about the existing commands, so
    that adding it is purely additive.

## Implementation Decisions

### A rank of groups, not a new aggregation

`compare` is deliberately built as a *presentation layer over the existing
aggregate*. It calls `AggregateResult::from_filtered_refs(records, group_by)` —
the exact function `usage` calls — and then derives two things that function
does not: an ordering and a set of shares.

This is the load-bearing decision. If `compare` re-implemented the summation it
would eventually disagree with `usage` about a group's tokens, and two commands
showing different totals for the same data is worse than not shipping the
command. By ranking the existing `Group` values, the two commands *cannot*
disagree — the invariant is structural, not a matter of keeping two code paths
in sync.

### The sort key is explicit and total

`--sort-by` selects one of `tokens` (default), `cost`, `sessions`, `messages`.

- `tokens` ranks by the **sum of the four token fields** (the group's total
  token count), descending.
- `cost` ranks by the group's `cost`, descending. A group with `cost = None`
  (mixed-source, or a source that does not record cost, e.g. Claude Code) sorts
  **after** every group with a cost, because "no cost data" is not "zero cost"
  and must not be ranked as if it were cheap.
- `sessions` ranks by session count, `messages` by message count, descending.

Ties are broken by the group key, ascending (lexicographic), so the ordering is
**total and deterministic**: the same records always produce the same byte
order, which is what makes the output diffable and the tests exact. A sort that
left ties in map order would reorder rows between runs and quietly break the
"same data, same bytes" property the rest of the tool keeps.

### Shares are computed from the same totals the aggregate reports

For each ranked group, `compare` reports:

- `share` — the group's *sort metric* as a percentage of the grand total for
  that metric. When sorting by `cost`, this is the group's cost over the sum of
  the *cost-reporting* groups' cost (groups with no cost are excluded from the
  denominator and shown as `—`, not `0%`), because including phantom zeros would
  understate every other group's share.
- `token_shares` (and per-field shares for input/output/cache_read/cache_write)
  — always by token count, independent of the sort key, because token shares are
  meaningful regardless of how the table is ordered.

Percentages are rounded to one decimal place, the same convention `diff` uses.
When the denominator is zero (an empty result, or no group reports cost while
sorting by cost) the share is `None`, rendered `—`, never a division by zero and
never `0%`.

### `--top N` folds the tail into `(others)`

`--top N` keeps the first `N` ranked groups verbatim and folds every remaining
group into a single synthetic `(others)` row, placed last. The `(others)` row:

- sums the folded groups' sessions, messages, tokens, and (when they are all
  cost-reporting and single-source) cost;
- carries the summed share, so the visible shares still add to 100%;
- sorts by construction last, regardless of the metric — it is a remainder, not
  a competitor.

Edge cases are explicit: `--top` absent means no fold (every group is shown).
`--top 0` is treated as "no limit" — folding everything into one `(others)` row
would be a legal but useless output, and rejecting `0` would be a surprising
error for a count. `--top N` with `N >= group count` emits no `(others)` row,
because nothing was folded. A `(others)` row aggregating groups from multiple
sources reports `cost = None`, exactly like any other mixed-source group.

### `src/compare.rs` is a pure module

```rust
pub enum SortBy { Tokens, Cost, Sessions, Messages }

pub struct RankedGroup {
    pub key: String,
    pub rank: usize,
    pub is_others: bool,
    pub sessions: usize,
    pub messages: usize,
    pub tokens: TokenBreakdown,
    pub cost: Option<f64>,
    pub share_pct: Option<f64>,
    pub input_share_pct: Option<f64>,
    pub output_share_pct: Option<f64>,
    pub cache_read_share_pct: Option<f64>,
    pub cache_write_share_pct: Option<f64>,
}

pub fn rank(
    groups: &[Group],
    grand_tokens: u64,
    sort_by: SortBy,
    top: Option<usize>,
) -> Vec<RankedGroup>;
```

`rank` reads no clock and touches no I/O — it takes an already-computed group
list and returns the ranked view. The ordering, the tiebreak, the share
arithmetic, and the `(others)` fold are therefore all unit tests with
hand-built groups and exact expected orderings, no fixture or binary needed.

The grand totals for the *share denominators* are passed in rather than
recomputed, so the denominator is provably the same number the aggregate
reports (the caller threads `AggregateResult`'s own totals through), closing the
same drift hole the ranking decision closes for the numerators.

### CLI surface

- New `Command::Compare(CompareArgs)` in `src/cli.rs`.
- `CompareArgs` implements the three existing shared traits rather than copying
  their accessors: `FilterArgs`, `SourcePathArgs`, `ExplainArgs`.
- New fields: `--sort-by` (`ValueEnum`, default `tokens`), `--top` (`Option<usize>`),
  plus the shared `--group-by`, `--json`, `--csv`.
- `compare` has **no** `--since` override of its own beyond the shared
  `FilterArgs`; the shared window flags apply as they do everywhere, and
  `--last` is the common entry point.
- `--json` and `--csv` are mutually exclusive, validated before any source is
  read, exactly as `diff` and `trend` validate them.

### Output shapes

`--json` is an object, not a bare array, so the funnel and the ranking can
coexist:

```json
{
  "group_by": "project",
  "sort_by": "tokens",
  "grand_tokens": 6840000,
  "rows": [
    {
      "rank": 1,
      "key": "/home/hunter/experimental/a",
      "sessions": 42, "messages": 318,
      "tokens": {"input": 1000000, "output": 200000, "cache_read": 2000000, "cache_write": 1000000},
      "cost": 1.82,
      "share_pct": 61.3,
      "token_shares": {"input": 55.2, "output": 61.0, "cache_read": 66.1, "cache_write": 58.0}
    }
  ],
  "diagnostics": { "loaded": 292, "stages": [...], "matched": 42, "blamed": null }
}
```

`diagnostics` obeys spec 0020's rule verbatim: present only when the result is
empty or `--explain` was passed, so an ordinary non-empty run gains no key.
`compare` is new, so a non-empty output is not byte-identical to anything
pre-existing, but the *diagnostics* field is the same contract the other
commands honor.

`--csv` emits a bare header plus one line per ranked group, to **stdout**; a
`--explain` funnel goes to **stderr**, keeping the body parseable — the
asymmetry spec 0020 deliberately established.

`cost` renders `—` (and `null` in JSON) for a group with no cost data, never
`0`. In CSV the same cell is empty, because CSV has no `—` convention and a
blank reads as "no data" to a spreadsheet.

## User Interface

Terminal table:

```
╭ llmhelper compare ────────────────────────────────────────────────────────╮
│ window: last 30d   group: project   sort: tokens                    rows 6 │
╰───────────────────────────────────────────────────────────────────────────╯
 rank  project                         sessions  messages    tokens   share    cost
    1  /home/hunter/experimental/a          42       318    4.2M    61.3%  1.820000
    2  /home/hunter/experimental/b          18       120    1.5M    21.9%  0.610000
    3  /home/hunter/experimental/c           9        44  980.1K    14.1%       —
  ---  (others)                              3        12  184.0K     2.7%       —
```

- Token figures use the existing `format_tokens` (K/M/B), so a compare row is
  readable at the same scale as a `usage` row.
- The header names the resolved window, the grouping dimension, the sort metric,
  and the row count.
- The `share` column is the sort metric's share; when sorting by cost, groups
  with no cost show `—` in both `cost` and `share`.
- The `(others)` row is visually separated and always last, labelled
  `(others)`, so it can never be mistaken for a real project.
- Group keys that are paths are printed in full; the tool does not truncate,
  because a truncated path is ambiguous and the terminal will scroll.

## Error Handling

| Condition | Behaviour |
|---|---|
| `--last` missing (and no other window flag) | Clap error, exit 1 |
| `--last` not a valid duration | error naming the flag (`invalid --last ...`), exit 1 |
| `--json` and `--csv` both set | validation error, exit 1 |
| `--sort-by` not one of the four keywords | Clap `ValueEnum` error, exit 1 |
| `--top` not a non-negative integer | Clap error, exit 1 |
| Zero matching records | exit 0; empty ranked output (header + no rows), `--explain` reports the funnel |
| Source directory missing | degrades with a `SourceStatus` error, zero records, same as `usage` |

Exit code stays `0` for an empty rank. Narrowing to nothing is a legitimate
result (spec 0020), and "no groups to rank" is a valid leaderboard.

## Testing Decisions

- **Ranking is unit-tested in `src/compare.rs`** with hand-built `Group`s and
  explicit grand totals: descending order per sort key, the key tiebreak making
  ties deterministic, `None`-cost groups sorting after cost-bearing ones when
  sorting by cost, shares summing to ~100%, share `None` when the denominator is
  zero, the `(others)` fold summing correctly and landing last, and the
  `--top 0` / `--top >= n` edge cases.
- **The CLI seam is integration-tested** in `tests/compare.rs` over the shared
  fixture tree, asserting only external behavior: rows are ordered by the chosen
  metric, `--top N` produces exactly N+1 rows (or N when nothing is folded),
  the `(others)` row's tokens equal the sum of the folded groups, `--group-by`
  changes the visible keys, `--json` parses and its `rows` length and `rank`
  sequence are correct, `--csv` body is a parseable header plus `n` lines with
  the funnel on stderr, mutually-exclusive flags error, and a missing `--last`
  errors.
- **Source-scoped cost gets its own test**: a compare mixing a cost-reporting
  source and Claude Code shows cost `—` for the mixed group and a real share
  only among the cost-bearing groups when sorting by cost.
- **Source-path isolation.** Every test passes all four override flags; tests
  asserting an empty result point all four at nonexistent temp paths, so a
  developer's real `~/.claude/projects` cannot leak in (the trap specs 0018,
  0019, and 0022 all recorded).
- **Empty is exit 0**, asserted explicitly, so the "narrowing to nothing is not
  an error" contract of spec 0020 is pinned for this command too.
- **Determinism is asserted**: two runs with the same input produce identical
  output bytes, pinning the tiebreak.

## Out of Scope

- **Two-window or time-series comparison.** That is `diff` (spec 0002) and
  `trend` (spec 0022); `compare` ranks groups *within one window* and does not
  put a time axis on anything.
- **Charts / sparklines / bar graphs.** The terminal output is a table. A bar
  rendering of shares is a presentation feature that can be layered on later
  without touching the data model.
- **Cross-metric composite scores.** Ranking is by exactly one metric, chosen
  explicitly. A blended "impact score" would be unauditable and is out of scope.
- **Cost aggregation across sources.** `--sort-by cost` ranks and shares cost
  only within cost-reporting, single-source groups, honoring ADR 0001. A
  cross-source "total spend" is not added here or anywhere.
- **Interactive TUI.** `compare` ships table/JSON/CSV only, like `trend`.
- **Budget evaluation.** `compare` does not evaluate budgets; that is `watch`
  (spec 0019) and `usage`/`report` (spec 0015).

## Further Notes

- The distinction from `usage --group-by` is *ordering and share*, not a
  different aggregation. A future reader may be tempted to fold `compare` back
  into `usage` with a `--sort` flag; the reason not to is that `usage`'s output
  is the raw grouped table (and is byte-stable for existing consumers), while
  `compare` is a derived ranked view with its own JSON shape. Keeping them
  separate keeps both contracts simple.
- The `(others)` fold is the load-bearing presentation decision. Without it a
  30-project month is an unscannable wall; with it the head is legible and the
  tail is still accounted for, which is why the fold carries shares rather than
  being a mere truncation.

---
id: 0022
title: "trend — usage bucketed over time"
status: done
created: 2026-09-15
updated: 2026-09-25
triage: done
---
## Problem Statement

Every read command in this tool shows a **total** over a window. `usage` sums a
week into one number per group; `diff` contrasts two windows; `report` prints
one summary table. None of them answers the question that actually drives a
decision: *when* did the usage happen?

The gap is visible in the questions users ask. "Did I blow my budget today, or
has this been accumulating all week?" "Is this project's cost a steady burn or
one runaway session?" "Which day should I stop worrying about?" A single total
cannot distinguish a flat 300K/day across a week from one 2M-token afternoon
and six quiet days — and those two weeks call for opposite responses.

Spec 0002 already noted this hole and parked it: "**Diff by date** (day-by-day
time series): out of scope; a single prev-vs-curr comparison only." The
calendar work (specs 0017/0018) then built the vocabulary for "what is a day" —
`WindowMode`, `local_midnight`, `calendar_bucket_before` — explicitly so the
commands would **agree** on bucket boundaries. What is missing is not the
calendar math, it is the command that walks a sequence of days instead of
summing them.

## Solution

Add a subcommand, `llmhelper trend`, that splits a window into equal, aligned
time buckets and reports the filtered records in each one.

```bash
llmhelper trend --last 30d --bucket 1d
llmhelper trend --last 30d --bucket 1d --group-by project --json
llmhelper trend --last 12w --bucket 1w --source opencode --explain
```

The output is one row per bucket, oldest first, with every bucket present —
including buckets that hold no records.

```
bucket                 sessions  messages  input   output  cache_read  cache_write  cost
2026-08-17                   0         0      0        0           0            0      —
2026-08-18                   2        11  120.4K    31.2K       80.1K         4.0K  0.410000
2026-08-19                   0         0      0        0           0            0      —
...
2026-09-15                   1         4   18.9K     5.1K       12.0K         0.9K  0.060000
```

`trend` reuses the pieces that already exist rather than introducing a second
data path:

- The **records** come from the same `Registry::load_all` the other read
  commands use.
- The **filter** is the same `Filter` (window → project → model → source), built
  from the same `FilterArgs` accessors, so `--explain` and the funnel behave
  identically.
- The **bucket boundaries** come from the shared `local_midnight` /
  `calendar_bucket_before` helpers (spec 0017) — `trend` computes no calendar
  math of its own.
- The **aggregation** is the same `AggregateResult::from_filtered_refs`, applied
  once per bucket instead of once overall.

What `trend` adds on top:

1. **Aligned, whole buckets.** The window is not `[now - last, now]` sliced into
   equal parts. It is a sequence of **whole** bucket starts, ending with the
   bucket `now` falls in. A partial leading bucket is never emitted.
2. **Zero-filled gaps.** A bucket with no records is a row of zeros, not an
   omission. This is the entire point of a trend: "I did nothing that day" and
   "there is no data for that day" must be distinguishable, and only an explicit
   row can say the former.
3. **Per-bucket cost, source-scoped.** Cost is summed only within a bucket that
   drew from a single source; a bucket mixing sources reports `—`, honoring ADR
   0001 exactly as `usage` does.

## User Stories

1. As a user, I want `llmhelper trend --last 30d --bucket 1d` to print one row
   per local day, so that I can see the shape of a month rather than its total.
2. As a user, I want buckets ordered oldest-first, so that the output reads in
   the same direction as time on a chart.
3. As a user, I want a day with no usage to appear as a zero row, so that a gap
   in activity is visible as a gap rather than silently closed up.
4. As a user, I want every bucket to be a **whole** local day, so that the last
   row is directly comparable to the ones above it instead of being a partial
   slice of the current day.
5. As a user, I want the bucket boundaries to be computed by the same code that
   `usage --calendar` and `diff --calendar` use, so that "a day" means one thing
   across the tool.
6. As a user, I want `--project`/`--source`/`--model` to narrow the trend to one
   dimension's slice, so that I can trend a single project over time.
7. As a user, I want `--source`, `--project`, and `--model` to filter before
   bucketing, so that a trend is measured on exactly the slice I asked for.
8. As a user, I want `--last` to be required, so that running `trend` bare gives
   an actionable error rather than a trend of an undefined span.
9. As a user, I want `--bucket` limited to the calendar keywords `1d`/`1w`/`1mo`,
   so that a bucket that does not align to a local day boundary is rejected
   loudly instead of silently misaligned.
10. As a user, I want `--bucket` to be required, so that I am never guessing
    what granularity a bare `trend` chose for me.
11. As a user, I want `trend --json` to emit a structured array of buckets, so
    that a script can chart the result without parsing a table.
12. As a user, I want `trend --csv` to emit a bare header plus one line per
    bucket, so that the output drops straight into a spreadsheet.
13. As a user, I want `--explain` to report the same funnel every other read
    command reports, so that "why is this trend empty" has one answer.
14. As a user, I want an empty result to still print every bucket row (or the
    full structured array) rather than nothing, so that "a month of zero usage"
    and "a broken invocation" do not render alike.
15. As a user, I want a budget's window to be unaffected by `trend`, so that
    adding this command changes nothing about the existing ones.
16. As a user, I want a Source that fails to load to degrade exactly as it does
    in `usage` (a `SourceStatus` error, zero records for that source), so that
    one broken source does not fail the whole trend.
17. As a user, I want `--json` and `--csv` to be mutually exclusive, so that I
    cannot ask for two encodings at once.
18. As a user, I want a bucket that mixes sources to report its cost as `—`
    rather than a summed-across-sources number, so that the per-source cost
    invariant is never violated.

## Implementation Decisions

### The bucket sequence is derived, not sliced

The central decision is that `trend` does **not** do
`[now - last, now]` divided into `last / bucket` equal parts. That produces a
ragged first bucket and a ragged last one (a 30-day window ending mid-afternoon
would emit a half-day at each end), and the rows would then not be comparable —
which defeats the purpose.

Instead the sequence is built backwards from `now`'s own bucket start:

```
anchor      = local_midnight(now)                 # the bucket now falls in
starts[i]   = calendar_bucket_before(anchor, i * bucket_days)   for i = 0..n
bounds[i]   = (starts[i], starts[i] + bucket_days)  # half-open
```

where `n = ceil(last_days / bucket_days)`, so the series covers at least the
requested span. Every bound lands on local midnight, every bucket is exactly
`bucket_days` local days wide, and `calendar_bucket_before` handles the DST
transitions (a local day is not always 86 400 seconds) because it steps in
**local-day** units. This is why `trend` reuses the spec-0017 helpers rather
than doing `now - Duration::days(i)` arithmetic, which would drift across a DST
boundary.

The **last** bucket is the current, still-open one: it runs from its local
midnight to `now`. That is deliberate and it is the only partial bucket,
because "today so far" is genuinely partial and pretending otherwise would
require a second vocabulary for "the day that has not finished".

`--bucket` must divide the window sensibly; a `--bucket 1mo` (30 local days)
with `--last 7d` yields a single bucket covering the last 30 days. That is not
an error — it is the honest consequence of asking for a bucket wider than the
window, and it is reported by the row's own bounds.

### `src/trend.rs` is a pure module

```rust
pub struct Bucket { pub since: DateTime<Utc>, pub until: DateTime<Utc> }
pub fn bucket_bounds(now: DateTime<Utc>, last_days: u32, bucket_days: u32) -> Option<Vec<Bucket>>;
pub fn bucketize(records: &[&Record], buckets: &[Bucket]) -> Vec<BucketRow>;
```

`bucket_bounds` reads no clock and touches no I/O, so the whole boundary
question — alignment, DST, the partial last bucket, the count of rows — is a
unit test with explicit instants. `bucketize` places each record by
`started_at` in the first bucket whose half-open `[since, until)` contains it; a
record falls in exactly one bucket by construction, so buckets never
double-count. Records outside every bucket (which cannot happen from the filter
that built the window, but is not enforced by types) are dropped rather than
being force-fit into an edge bucket, and a test pins that.

### `BucketRow` reuses `AggregateResult`, not a parallel type

Each bucket is aggregated with `AggregateResult::from_filtered_refs` — the same
function the other commands call, so sessions, messages, tokens, and the
source-scoped cost rule cannot drift between `trend` and `usage`. A `BucketRow`
pairs a bucket's bounds with that result's grand totals.

Cost follows the existing invariant exactly: within one bucket, cost is `Some`
only when every contributing record reports cost **and** they share a source;
otherwise `—`. A bucket mixing a Claude record (no cost) with an OpenCode one
therefore reports `—`, never `$0.41`, which is the failure mode ADR 0001 exists
to prevent.

### `--bucket` is a calendar keyword, not a duration

`--bucket` accepts exactly `1d`, `1w`, `1mo`, via the shared
`domain::window::calendar_days`. A duration like `4h` has no local-day
alignment and is rejected with an error naming the flag and the keywords, the
same shape `--calendar` gives a non-keyword (spec 0017). This keeps the bucket
grid honest: sub-day buckets would need a different boundary rule (they cannot
align to midnight), and silently choosing one would put `trend` and `--calendar`
in conflict about what a boundary is.

### CLI surface

- New `Command::Trend(TrendArgs)` in `src/cli.rs`.
- `TrendArgs` implements the three existing shared traits rather than copying
  their accessors: `FilterArgs`, `SourcePathArgs`, `ExplainArgs`.
- New fields: `--last` (required), `--bucket` (required), plus the shared
  `--json`, `--csv`.
- `trend` has **no** `--group-by`. A bucket row is a bucket's totals; there is
  one row per bucket and no place to put a per-dimension breakdown. Slicing a
  trend to one project/model/source is what `--project`/`--model`/`--source`
  already do (they filter *before* bucketing), so a `--group-by` flag would
  either do nothing visible or silently redefine what a row means. Omitting it
  keeps "one row = one bucket" true.
- `trend` has **no** `--since`: a trend needs a bucket-aligned start, and
  `--since` names an arbitrary instant that would immediately reintroduce the
  ragged-first-bucket problem the whole design avoids. `--last` is the only
  entry point, and it is required.
- `--json` and `--csv` are mutually exclusive, validated before any source is
  read, exactly as `diff` validates them.

### Output shapes

`--json` is an object, not a bare array, so the funnel and the bucket list can
coexist:

```json
{
  "buckets": [
    {"since": "2026-08-17T00:00:00Z", "until": "2026-08-18T00:00:00Z",
     "sessions": 0, "messages": 0,
     "tokens": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0},
     "cost": null}
  ],
  "diagnostics": { "loaded": 292, "stages": [...], "matched": 0, "blamed": null }
}
```

`diagnostics` obeys spec 0020's rule verbatim: present only when the result is
empty or `--explain` was passed, so an ordinary non-empty run gains no key. A
non-empty trend is **not** byte-identical to any pre-existing output because
`trend` is new, but the *diagnostics* field is the same contract the other
commands honor, so a consumer that understands `usage --json` understands this
one.

`--csv` emits a bare header plus one line per bucket, to **stdout**; a `--explain`
funnel goes to **stderr**, keeping the body parseable — the asymmetry spec 0020
deliberately established.

`cost` renders `—` (and `null` in JSON) for a bucket with no cost data, never
`0`. In CSV the same cell is empty, because CSV has no `—` convention and a
blank reads as "no data" to a spreadsheet.

### Zero-filled buckets and the empty result

A bucket with no records is emitted as a zero row in every output mode. An
entire trend with zero matching records therefore still prints `n` rows: this
is the one place the tool deliberately prints a full table of zeros instead of
an empty result, because the number and identity of the rows is itself the
information. `--explain` still reports the funnel, so "all zeros because
nothing matched" remains distinguishable from "all zeros because there was no
activity".

## User Interface

Terminal table:

```
╭ llmhelper trend ──────────────────────────────────────────────────────────╮
│ window: last 30d   bucket: 1d   group: source       2026-08-17 → 2026-09-15│
╰───────────────────────────────────────────────────────────────────────────╯
 bucket       sessions  messages   input   output  cache_read  cache_write  cost
 2026-08-17          0         0       0        0           0            0     —
 2026-08-18          2        11  120.4K    31.2K       80.1K         4.0K  0.410000
 2026-08-19          0         0       0        0           0            0     —
```

- Token figures use the existing `format_tokens` (K/M/B), so a trend row is
  readable at the same scale as a `usage` row.
- The header names the resolved span, not the flags: the first and last bucket
  bounds, so a DST shift or a clamped tail is visible rather than implied.
- Rows are oldest-first; there is no sort flag, because a time series that can
  be arbitrarily reordered stops reading as time.
- A bucket whose bounds contain `now` is the partial one; it is not specially
  marked in the table, because its `until` is already `now` in the JSON and the
  header's end bound makes it explicit. (A marker would be a second way to say
  the same thing and could disagree with the data.)

## Error Handling

| Condition | Behaviour |
|---|---|
| `--last` missing | Clap error, exit 1 (`--last is required`) |
| `--bucket` missing | Clap error, exit 1 (`--bucket is required`) |
| `--last` not a valid duration | error naming the flag (`invalid --last ...`), exit 1 |
| `--bucket` not `1d`/`1w`/`1mo` | error naming the flag and the keywords, exit 1 |
| `--json` and `--csv` both set | validation error, exit 1 |
| Zero matching records | exit 0; all buckets print as zero rows; `--explain` reports the funnel |
| Source directory missing | degrades with a `SourceStatus` error, zero records, same as `usage` |
| Calendar bucket unanchorable (DST gap) | `None` propagates to a clear error, exit 1 (same contract as `window_bounds`) |

Exit code stays `0` for an empty trend. Narrowing to nothing is a legitimate
result (spec 0020), and a trend of nothing is still a trend.

## Testing Decisions

- **Boundary math is unit-tested in `src/trend.rs`** with explicit `now`
  instants derived from `Local` (the `tests` helper spec 0017 established), so
  the tests hold in any timezone and at any DST state: bucket count for a
  non-dividing window, every bound on local midnight, the last bucket ending at
  `now`, oldest-first order, and a `--bucket` wider than `--last` yielding one
  bucket.
- **Bucketing is unit-tested** with hand-built `Record`s: a record on a lower
  bound is included, one on the upper bound is excluded (half-open), a record
  outside every bucket is dropped, and no record is counted twice.
- **Source-scoped cost** gets its own unit test: a bucket holding one
  cost-reporting and one non-cost-reporting record reports `None`, and a bucket
  whose records all share a cost-reporting source reports the sum.
- **The CLI seam is integration-tested** in `tests/trend.rs` over the shared
  fixture tree, asserting only external behavior: row count equals the expected
  bucket count, oldest-first ordering, zero-fill rows present, `--group-by`
  changes the visible shape, `--json` parses and its `buckets` length matches
  the window, `--csv` body is a parseable header plus `n` lines with the funnel
  on stderr, mutually-exclusive flags error, and a missing required flag errors.
- **Source-path isolation.** Every test passes all four override flags; tests
  asserting an empty result point all four at nonexistent temp paths, so a
  developer's real `~/.claude/projects` cannot leak in (the trap spec 0018 and
  0019 both recorded).
- **Empty is exit 0**, asserted explicitly, so the "narrowing to nothing is not
  an error" contract of spec 0020 is pinned for this command too.

## Out of Scope

- **Arbitrary bucket widths.** Sub-day (`4h`) and non-calendar buckets are out
  of scope; they would need a different alignment rule and would fork the
  definition of a boundary.
- **Interactive TUI.** `trend` ships table/JSON/CSV only. A charting TUI is a
  separate concern and is not needed to answer "when did it happen".
- **Charts / sparklines.** The terminal output is a table. Rendering an
  in-terminal bar chart is a presentation feature that can be layered on later
  without changing the data model.
- **`--since` / `--until`.** Only a trailing `--last` window is supported, for
  the bucket-alignment reason above.
- **Cumulative / rolling-average columns.** Each row reports its own bucket
  only; derived series are a consumer-side concern.
- **Budget evaluation.** `trend` does not evaluate budgets; that is `watch`
  (spec 0019) and `usage`/`report` (spec 0015).

## Further Notes

- `diff` remains the two-window contrast; `trend` is the sequence. They share
  the calendar vocabulary but answer different questions, and neither subsumes
  the other.
- The zero-fill decision is the load-bearing one. A future "skip empty buckets"
  flag would be tempting, but it would make two trends of the same data
  non-comparable by row position, which is the property most consumers rely on.

---
id: 0017
title: "diff — calendar-aligned comparison windows"
status: done
created: 2026-09-13
updated: 2026-09-13
triage: done
---

## Problem Statement

`diff` compares two **sliding** windows anchored to `now`: with `--last 7d
--prev 7d` it contrasts `[now-7d, now]` against `[now-14d, now-7d]`. That is the
right comparison for "am I trending up?", but it cannot answer the questions a
user actually asks at a boundary: "how does **today** compare to **yesterday**?",
"how does **this week** compare to **last week**?"

Run `diff --last 1d --prev 1d` at 09:00 and "today" silently means "the last 24
hours", which spans two calendar days — this morning plus all of yesterday
afternoon and evening. Run the same command at 09:00 tomorrow and the window has
slid again, so the two runs describe different days and cannot be compared.
There is no way to say "the calendar day" or "the calendar week".

The project already has this vocabulary: `budget` (spec 0015) recognizes the
calendar keywords `1d`, `1w`, and `1mo`, anchored to **local** midnight, so a
"daily" budget means the user's day, not UTC's. `diff` should speak the same
language rather than invent a second one.

## Solution

Add a `--calendar` flag to `diff`. With it, `--last` and `--prev` are read as
**calendar buckets** instead of rolling durations, and the two windows are
snapped to **adjacent, non-overlapping local-midnight boundaries**:

- The **current** window runs from the start of the current bucket to `now`.
- The **previous** window runs from the start of the *previous* bucket of the
  same length to the start of the current bucket.

So `diff --calendar --last 1d --prev 1d` compares *today so far* against *all of
yesterday*, with the boundary at **local midnight** (the same anchor `budget`
uses). `diff --calendar --last 1w --prev 1w` compares the trailing 7 local days
including today against the 7 days before them.

Without `--calendar`, `diff` behaves exactly as it does today — the sliding
window is unchanged, and the flag is purely additive.

## User Stories

1. As a user, I want `diff --calendar --last 1d --prev 1d` to compare today
   against yesterday, so that a daily standup comparison means calendar days,
   not "the last 24 hours".
2. As a user, I want `diff --calendar --last 1w --prev 1w` to compare this week
   (the trailing 7 local days) against the prior 7, so that weekly reviews line
   up with the day boundaries I live in.
3. As a user, I want `--calendar` to anchor to **local** midnight, so that
   "today" matches the same day my `budget` daily ceiling resets on.
4. As a user, I want the two calendar windows to be **adjacent and
   non-overlapping**, so a session is counted in exactly one of them.
5. As a user, I want `--calendar --last 1d --prev 1w` to compare today against
   the 7 calendar days before it, so that I can contrast a short bucket with a
   longer baseline.
6. As a user, I want the TUI header to show the *actual* boundary timestamps and
   the calendar label (`1d`, `1w`, `1mo`) rather than a fabricated "24h"
   duration, so that the header does not lie about a partial current bucket.
7. As a user, I want the JSON output to carry the same window bounds in
   `windows.current`/`windows.previous`, so that a script sees the calendar
   boundaries I see in the TUI.
8. As a user, I want `--calendar` with a duration that is not a calendar
   keyword (e.g. `--calendar --last 4h`) to fail with a clear error naming the
   offending value, so that I am not silently given a sliding window labeled as
   calendar.
9. As a user, I want `--calendar` to be usable with every existing flag
   (`--group-by`, `--source`, `--project`, `--model`, `--json`, `--csv`), so
   that calendar alignment composes with the filters I already use.
10. As a maintainer, I want the local-midnight bucket math to live in **one**
    place shared with `budget`, so that `budget` and `diff` can never drift on
    what "a calendar day" means.
11. As a maintainer, I want `diff` without `--calendar` to be byte-for-byte
    unchanged, so that the flag's addition carries no risk to existing users.

## Implementation Decisions

- **`--calendar` is a boolean flag on `DiffArgs`.** Absent, `diff` is exactly as
  before. It is not a mode enum; there is one extra behavior and one existing
  behavior.
- **Shared bucket math.** The local-midnight anchoring currently inlined in
  `BudgetWindow::lower_bound` is extracted to a pure domain helper in a new
  `src/domain/window.rs`:

  ```rust
  /// Start of the local calendar day containing `now`, as UTC.
  pub fn local_midnight(now: DateTime<Utc>) -> Option<DateTime<Utc>>;

  /// Start of a trailing calendar bucket of `days` days ending on `now`'s local
  /// day: `local_midnight(now) - (days - 1)`. `days == 1` is "today".
  pub fn calendar_bucket_start(now: DateTime<Utc>, days: u32) -> Option<DateTime<Utc>>;

  /// Start of the `days`-long local bucket immediately *before* `anchor`'s
  /// local day, stepping back in local-day units so a DST transition cannot
  /// shift the boundary off midnight.
  pub fn calendar_bucket_before(anchor: DateTime<Utc>, days: u32) -> Option<DateTime<Utc>>;

  /// The calendar keyword vocabulary shared with `budget`: `1d` → 1, `1w` → 7,
  /// `1mo` → 30; anything else is `None`.
  pub fn calendar_days(s: &str) -> Option<u32>;
  ```

  `BudgetWindow::lower_bound` / `parse` delegate to these, so budget behavior is
  unchanged and there is a single definition of a calendar day.
- **One window-pair builder, driven by a `DiffMode`.** `src/diff.rs` gains a
  small enum and one pure function that both the CLI path and the TUI use:

  ```rust
  pub enum DiffMode {
      Sliding { last: chrono::Duration, prev: chrono::Duration },
      Calendar { last_days: u32, prev_days: u32 },
  }

  /// (prev_since, prev_until, curr_since, curr_until) at `now`, or `None` if a
  /// calendar bucket cannot be anchored.
  pub fn window_pair(mode: DiffMode, now: DateTime<Utc>) -> Option<WindowPair>;
  ```

  The `Sliding` arm reproduces the old arithmetic exactly
  (`prev_until = now - last; curr_since = prev_until - prev`); the `Calendar`
  arm builds the adjacent buckets. `main.rs`'s `window_filters` and the TUI's
  `refresh_windows` both call `window_pair`, so the two can never disagree.
- **Window construction in calendar mode.** Given `last` with `dl` days and
  `prev` with `dp` days:

  ```
  curr_since = calendar_bucket_start(now, dl)     // today's bucket start
  curr_until = now
  prev_until = curr_since
  prev_since = calendar_bucket_before(curr_since, dp)
  ```

  This makes the windows adjacent (`prev_until == curr_since`) and the previous
  window exactly `dp` calendar days long, with its start on local midnight even
  across a DST transition.
- **Parsing.** In calendar mode both `--last` and `--prev` must be calendar
  keywords (`1d`/`1w`/`1mo`); any other string is a loud error naming the flag
  and the value. This prevents a rolling duration (`4h`) from being silently
  treated as a calendar bucket. In sliding mode parsing is unchanged.
- **Window labels for the header.** `DiffApp` stores the `DiffMode` plus two
  label strings. Sliding mode renders a compact duration via `fmt_window_len`
  (moved out of the renderer into `diff.rs` so it is unit-testable); calendar
  mode renders the raw `--last`/`--prev` keyword text, because the current
  bucket is partial (`local midnight → now`) and a duration would misstate it.
- **No change to `Filter`, `Source`, `Record`, or `AggregateResult`.** The
  windows are still expressed as `since`/`until` bounds; only their derivation
  changes.

## User Interface

```
llmhelper diff --calendar --last 1d --prev 1d
llmhelper diff --calendar --last 1w --prev 1w --group-by project
llmhelper diff --calendar --last 1d --prev 1w --json
```

- `--calendar` requires `--last` and `--prev`, as today.
- `--calendar --last <non-keyword>` → exit 1:
  `invalid --last for --calendar: '4h' (expected calendar window: 1d, 1w, 1mo)`.
- The TUI header shows `curr  09-13 00:00 → 09-13 14:32  (1d)` and
  `prev  09-12 00:00 → 09-13 00:00  (1d)` in local time.
- `--json` `windows.previous`/`windows.current` carry the resolved `since`/
  `until` instants, unchanged in shape from the sliding mode.

## Error Handling

| Scenario | Behavior |
|---|---|
| `--calendar` without `--last`/`--prev` | existing required-flag error, exit 1 |
| `--calendar --last 4h` | exit 1, names `--last` and the bad value |
| `--calendar --prev 2d` | exit 1, names `--prev` and the bad value (only `1d`/`1w`/`1mo`) |
| `--calendar` with zero records in either bucket | exit 0; rows all `new` or `removed` |
| `--calendar` + `--json`/`--csv` | works; format output unchanged |
| Local-midnight resolution fails (DST gap) | falls back to the earliest valid instant, as `budget` does |

## Testing Decisions

### Seam

The existing **`--json` CLI seam** against fixture directories, plus pure unit
tests for the new domain helpers and the calendar window-pair builder. TUI
header rendering gets a state-level unit test, matching how `diff_app` is
already tested.

### What makes a good test

- The shared helpers are asserted on **explicit timestamps** with an explicit
  local timezone offset, so the test does not depend on the machine's wall
  clock (mirroring `budget`'s `calendar_day_resets_at_local_midnight`).
- The calendar window pair is asserted to be **adjacent and non-overlapping**
  for equal and unequal `dl`/`dp`.
- Integration tests drive the real binary and assert on JSON `windows` bounds
  and on the error message for a non-keyword under `--calendar`.
- A regression test pins that **sliding mode is unchanged**: the same
  `--last/--prev` without `--calendar` produces the sliding boundaries.

### Test coverage

| Scenario | What it asserts |
|---|---|
| `local_midnight` on a known local offset | returns the UTC instant of local 00:00 |
| `calendar_bucket_start` for 1/7/30 days | start = local midnight minus (days-1) |
| equal buckets `1d`/`1d` | prev_until == curr_since; prev exactly 1 local day |
| unequal buckets `1d`/`1w` | prev is 7 days wide, ends at curr_since |
| non-keyword under `--calendar` | exit 1, message names flag + value |
| `--calendar --last 1d --prev 1d --json` | `windows` bounds at local midnight |
| `--calendar` TUI header | label shown, not a computed duration |
| sliding mode unchanged | no `--calendar` → boundaries equal today's behavior |
| `--calendar` + `--group-by project` | grouping still applies within buckets |

## Out of Scope

- **Arbitrary `--since` calendar ranges** (e.g. "since last Monday"). Calendar
  mode buckets `1d`/`1w`/`1mo`; a general date range remains a `usage`/`report`
  concern.
- **`diff` by date / time series.** Still a single prev-vs-curr comparison.
- **Historical diff** (comparing two arbitrary past ranges). Calendar mode is
  anchored to `now`.
- **Changing sliding-window behavior.** The default path is frozen.
- **Calendar alignment for `usage`/`report`.** This spec is scoped to `diff`;
  the single-window counterpart is spec 0018, and `budget` already has its own
  calendar windows.
- **A new calendar keyword vocabulary** beyond what `budget` recognizes. `1d`,
  `1w`, `1mo` only.

## Further Notes

- This removes the "Window alignment to calendar boundaries" entry from spec
  0002's Out of Scope, which named exactly this as a future enhancement.
- The extraction of `local_midnight`/`calendar_bucket_start` is the second time
  a time-window concept has been shared across commands (after `Filter`), and
  keeps "what is a day" from being defined twice.

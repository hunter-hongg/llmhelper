---
id: 0018
title: "usage / report — calendar-aligned windows"
status: done
created: 2026-09-14
updated: 2026-09-14
triage: done
---

## Problem Statement

`usage` and `report` scope their records with a single rolling window:
`--last 1d` means `[now - 24h, now]`. That is a fine way to ask "what happened
recently?", but it cannot answer the boundary question users actually ask:
"how much did I use **today**?", "what did this **week** cost?"

Run `llmhelper report --last 1d` at 09:00 and it reports *the last 24 hours* —
this morning plus all of yesterday afternoon and evening. Run it again at 09:00
the next day and the window has slid again, so the two reports describe
different spans and cannot be compared or totalled against a daily plan.
At 23:59 the same command shows "today plus the tail of yesterday"; one minute
later it shows a completely different set of records. There is no way to say
"the calendar day".

The project already has this vocabulary, and it is already shared:

- `budget` (spec 0015) defines a daily ceiling that resets at **local
  midnight**.
- `diff --calendar` (spec 0017) buckets its comparison windows at local
  midnight using the same keywords, via the shared pure helpers in
  `src/domain/window.rs` (`local_midnight`, `calendar_bucket_start`,
  `calendar_days`).

So there are now three different answers to "what is a day" a user can hold in
their head for three commands — rolling, for `usage`/`report`; calendar, for
`budget` and `diff --calendar`. `usage` and `report` should speak the language
the other two already speak.

## Solution

Add a `--calendar` flag to `usage` and `report`. With it, `--last` is read as a
**calendar bucket** instead of a rolling duration, and the window's lower bound
is snapped to **local midnight**:

- The window runs from the start of the trailing bucket to `now`, so
  `--calendar --last 1d` means *today so far* (local midnight → now),
  `1w` means the trailing 7 local days including today, and `1mo` means the
  trailing 30 local days.

`--calendar --last 1d` and a `1d` budget now describe **exactly the same
window**; a report that says "today: $4.80" can be read directly against a $5
daily ceiling.

This is the single-window counterpart of spec 0017: `diff` compares two
adjacent buckets, `usage`/`report` scope one. Both reuse the same bucket math,
so there remains exactly one definition of a calendar day in the codebase.

Without `--calendar`, `usage` and `report` behave exactly as they do today —
the rolling window is unchanged, and the flag is purely additive.

## User Stories

1. As a user, I want `usage --calendar --last 1d` to show *today's* usage
   starting at local midnight, so that the window matches the day I live in
   rather than a rolling 24 hours.
2. As a user, I want `report --calendar --last 1w` to cover the trailing 7
   local days including today, so that a weekly review aligns with day
   boundaries.
3. As a user, I want `--calendar --last 1d` to select **exactly** the records a
   `1d` budget evaluates, so that a displayed cost can be read directly against
   the ceiling without wondering whether the two windows disagree.
4. As a user, I want `--calendar` to anchor to **local** midnight, so that
   "today" matches the day my `budget` daily ceiling resets on.
5. As a user, I want `usage --calendar --last 1d --json` to emit the same
   records and totals as the TUI shows for that window, so that scripts and the
   UI agree.
6. As a user, I want `report --calendar --last 1d` to name the calendar window
   in its header (e.g. `last 1d (calendar, local midnight)`), so that a shared
   Markdown document does not misstate what it measured.
7. As a user, I want `--calendar` with a duration that is not a calendar
   keyword (e.g. `--calendar --last 4h`) to fail with a clear error naming the
   offending value, so that I am not silently given a rolling window labeled as
   calendar.
8. As a user, I want `--calendar` combined with `--since` to fail loudly, so
   that two conflicting lower bounds are never silently reconciled.
9. As a user, I want `--calendar` to compose with every existing filter
   (`--project`, `--model`, `--source`) and output mode (`--json`, `--csv`, the
   TUI), so that calendar alignment is orthogonal to scoping.
10. As a user, I want `usage --calendar --last 1d` to remain **live**: on each
    TUI refresh the bucket start stays pinned at local midnight while `now`
    advances, so a session left open across midnight rolls over instead of
    freezing at the window captured at startup.
11. As a user, I want `usage --calendar --last 1d` combined with a `1d` budget
    to produce **no** "clipped window" warning, so that the budget annotation
    does not claim the command loaded less than it did.
12. As a maintainer, I want the calendar-window derivation to live in **one**
    place shared by `budget`, `diff`, `usage`, and `report`, so that the four
    commands can never drift on what "a day" means.
13. As a maintainer, I want `usage`/`report` without `--calendar` to be
    byte-for-byte unchanged, so that the flag's addition carries no risk to
    existing users.

## Implementation Decisions

- **`--calendar` is a boolean flag on `UsageArgs` and `ReportArgs`.** Absent,
  both commands are exactly as before. It is not a new subcommand and not a mode
  enum at the CLI surface.

- **One shared window-mode type, moved out of `diff`.** `DiffMode` and
  `window_pair` currently live in `src/diff.rs` but describe a *window
  derivation*, not a diff: `Sliding { last, prev }` and
  `Calendar { last_days, prev_days }` each yield two bounds. Rather than add a
  parallel one-window type, the vocabulary moves to `src/domain/window.rs` as:

  ```rust
  pub enum WindowMode {
      Rolling { last: Duration },
      Calendar { days: u32 },
  }

  /// (since, until) for `mode` at `now`. A rolling window starts at `now -
  /// last`; a calendar window starts at the trailing bucket's local midnight.
  pub fn window_bounds(mode: WindowMode, now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)>;
  ```

  `diff` keeps its two-window shape by deriving `window_bounds`-equivalent
  bounds from the same helpers; `DiffMode`'s two arms become compositions of
  `WindowMode` `Rolling`/`Calendar` rather than a second implementation. The
  existing `calendar_bucket_start` / `calendar_bucket_before` / `calendar_days`
  helpers are unchanged.

- **Window construction.** For `Calendar { days }`:
  `since = calendar_bucket_start(now, days)`, `until = now`. For `Rolling { last }`:
  `since = now - last`, `until = now`. This is the same arithmetic `diff`'s
  sliding arm already uses, so the sliding path stays frozen.

- **Parsing.** In calendar mode `--last` must be a calendar keyword
  (`1d`/`1w`/`1mo`); anything else is a `--last` error naming the flag and the
  value, mirroring spec 0017's wording. `--calendar` together with `--since` is
  an error, because both name a lower bound and silently preferring one would
  hide the other.

- **`--calendar` requires `--last`.** Without `--last` there is no bucket to
  anchor; a bare `--calendar` is an error rather than an implied `1d`.

- **`Filter` is unchanged.** The window is still expressed as
  `since`/`until`; only its derivation changes. No new field, no new predicate.

- **Live windows in the `usage` TUI.** The TUI currently captures one `Filter`
  at startup and reuses it for every refresh. That is already *slightly* wrong
  for `--last` (the bound is frozen while `now` advances); in calendar mode it
  would be visibly wrong, because a session open across midnight would keep
  showing yesterday's bucket. The `usage` TUI therefore carries the window mode
  and rebuilds the window at each load, exactly as the `diff` TUI already does
  (spec 0017). The non-window predicates continue to be captured once.

- **`command_since` stays the single source for the budget-clipping bound.**
  In calendar mode it returns the bucket start; the budget evaluation receives
  the same instant, so a `1d` budget under `--calendar --last 1d` is measured
  over exactly the loaded window and no "clipped" note is derived.

- **`report` names the window honestly.** `ReportMeta.window` currently renders
  `last {last}`. In calendar mode it renders the keyword plus a calendar marker,
  because the window is a partial local day and a duration would misstate it.

- **No change to `Source`, `Record`, `AggregateResult`, or `export`/`search`.**
  `export` and `search` keep rolling windows in this spec.

## User Interface

```
llmhelper usage  --calendar --last 1d
llmhelper usage  --calendar --last 1w --group-by project
llmhelper report --calendar --last 1d
llmhelper report --calendar --last 1mo --output week.md
```

- `--calendar --last <non-keyword>` → exit 1:
  `invalid --last for --calendar: '4h' (expected calendar window: 1d, 1w, 1mo)`.
- `--calendar --since <ts>` → exit 1: `--calendar and --since are mutually
  exclusive`.
- `--calendar` without `--last` → exit 1: `--calendar requires --last`.
- The `report` header shows `last 1d (calendar, local midnight)` in place of
  `last 1d` so the shared document states the anchoring it actually used.
- The `usage` TUI footer/header is unchanged in shape; the window bounds the
  budget indicator compares against are those of the current bucket.

## Error Handling

| Scenario | Behavior |
|---|---|
| `--calendar --last 4h` | exit 1, names `--last` and the bad value |
| `--calendar --since <ts>` | exit 1, `--calendar and --since are mutually exclusive` |
| `--calendar` without `--last` | exit 1, `--calendar requires --last` |
| `--calendar` with zero records in the bucket | exit 0; empty totals, no error |
| `--calendar` + `--json`/`--csv` | works; format output unchanged |
| Local-midnight resolution fails (DST gap) | falls back to the earliest valid instant, as `budget` does |
| `--calendar --last 1d` + a `1d` budget | no clipping note; the windows coincide |

## Testing Decisions

### Seam

The existing **`--json` CLI seam** against fixture directories, plus pure unit
tests for the new domain helper(s). The `usage` TUI's window rebuild gets a
seam test using an injected `now`, matching how `diff_app` is already tested.

### What makes a good test

- Domain helpers are asserted on **explicit timestamps** derived from `Local`,
  never a hardcoded UTC offset, so the tests hold on any machine (mirroring
  `src/domain/window.rs` and `budget`).
- Integration tests drive the real binary with `--json` and assert on the
  records present at the bucket boundary, not merely on a rendered string.
- A regression test pins that **without `--calendar` the rolling window is
  unchanged** for the same `--last`.
- The budget-coincidence claim is tested directly: `--calendar --last 1d` with a
  `1d` budget produces no clipped-window note.

### Test coverage

| Scenario | What asserts |
|---|---|
| `window_bounds` rolling | `since == now - last`, `until == now` (unchanged arithmetic) |
| `window_bounds` calendar | `since == local_midnight(now) - (days-1)`, `until == now` |
| `--calendar --last 1d` JSON | a record at 23:00 yesterday is excluded; one at 00:30 today is included |
| `--calendar` non-keyword | exit 1, message names flag + value |
| `--calendar --since` | exit 1, mutual-exclusion message |
| `--calendar` without `--last` | exit 1, `requires --last` |
| rolling unchanged | same `--last` without `--calendar` yields the rolling bound |
| rolling ≠ calendar at a boundary | the two select different record sets at a known `now` |
| `report --calendar` header | header prints the calendar marker |
| `--calendar --last 1d` + `1d` budget | report contains no "only records from" clipping note |
| `usage` TUI window rebuild across midnight | bucket start stays at local midnight as `now` advances |

## Out of Scope

- **`export` and `search` calendar windows.** Their units differ (flat records,
  message hits); a future spec if wanted.
- **Arbitrary `--since` calendar ranges** ("since last Monday"). Buckets are
  `1d`/`1w`/`1mo` only, as in spec 0017.
- **A calendar summary in `usage`'s JSON payload beyond the records themselves.**
  `usage --json` reports its groups; it does not gain a `window` block here
  (`diff`'s JSON already has one).
- **Changing the rolling path.** Frozen and regression-tested.
- **A new calendar keyword vocabulary** beyond what `budget` recognizes.
- **Month-boundary (`1mo` = this calendar month) semantics.** `1mo` stays the
  trailing 30 local days, matching `budget` and `diff`.

## Further Notes

- This removes the "Calendar alignment for `usage`/`report`" entry from spec
  0017's Out of Scope, which named exactly this as the follow-up.
- Moving `DiffMode`/`window_pair`'s vocabulary into the shared `domain::window`
  module makes "what is a day" defined once for four commands — the same
  rationale that motivated extracting `local_midnight` for `budget`/`diff`.
- `--calendar --last 1d` and a `1d` budget sharing one window is the concrete
  user-visible payoff of that single definition: the number and the ceiling are
  measured over the same records by construction, not by coincidence.

---
id: 0019
title: "watch — a live, self-refreshing usage monitor"
status: done
created: 2026-09-14
updated: 2026-09-14
triage: done
---

## Problem Statement

Every command in this tool answers a question **once**. `usage` reads the
sources, renders a frame, and waits for a keypress; a background refresh exists
but it is silent — nothing on screen says when the data was read or when the
next read will happen, and the only way to see "right now" is to press `r`.

That leaves the most common question of a long agent run unanswered: *is my
spend climbing past the line I drew, and am I creeping toward the ceiling I set
this morning?* A user with an OpenCode budget and sessions fanning out across
projects has to re-run `usage --calendar --last 1d` by hand and eyeball the
numbers.

`usage --calendar --last 1d` already computes exactly the right window (spec
0018) and a `1d` budget already evaluates exactly the right records (spec
0015). What is missing is not the data but the **continuous, honest way to look
at it**: a view whose entire purpose is to keep re-reading the sources, show
what changed since the last read, and say plainly whether a budget has been
crossed.

## Solution

Add a seventh subcommand, `llmhelper watch`: a focused, continuously refreshing
spend monitor.

```bash
llmhelper watch
llmhelper watch --calendar --last 1d --interval 5
llmhelper watch --calendar --last 1d --budget opencode:5.00
```

`watch` reuses the pieces that already exist rather than introducing a second
data path:

- The **window** is the shared `WindowMode` / `window_bounds` vocabulary
  (spec 0017/0018), rebuilt from a freshly read `now` on every reload, so a
  calendar bucket stays pinned to local midnight as the clock advances.
- The **records and aggregation** come from the same
  `Registry::load_all` + `AggregateResult::from_records` calls `usage` uses.
- The **budget evaluation** is the same pure function from spec 0015,
  measured against the same `now` that built the window.

What `watch` adds on top:

1. **A visible clock.** The header always shows when the data was read
   (`updated 12:00:03`) and when the next read happens (`next 5s`), counted
   down on every frame. The refresh is no longer invisible.
2. **Deltas since the last read.** Every group row is annotated with what
   changed since the previous reload (`+1.5K`, `+$0.12`), and the header
   reports the elapsed span. The first frame shows `—`, because there is
   nothing to compare against yet — never a fabricated `+0`.
3. **Loud budget state.** The budget indicator is the *primary* header element
   when budgets exist, and a Source row with an `over` budget is marked with
   `⚠` using the same palette role the `usage` TUI already uses.
4. **No interactive table.** `watch` is a monitor, not a browser: there is no
   group cycling, no detail view, no selection. That is deliberate — those
   belong to `usage`, and a monitor that silently rebinds keys under a
   changing table is worse than one with three keys.

Keys are exactly `r` (reload now) and `q`/`Esc` (quit).

## User Stories

1. As a user, I want `llmhelper watch` to keep re-reading my sources on its
   own, so that I can leave it in a pane during a long run and look over at any
   moment without re-running a command.
2. As a user, I want the header to say **when** the data on screen was read, so
   that I can tell fresh data from a frame that failed to reload.
3. As a user, I want the header to show a **live countdown** to the next read,
   so that a monitor with a long interval is distinguishable from a frozen one.
4. As a user, I want each budget's spend shown with a progress-style
   `spend / ceiling` reading and its status, so that I can see how close I am
   to the line rather than just which side of it I am on.
5. As a user, I want a crossable budget to be visibly flagged, so that a
   crossed line is obvious at a glance from across the room.
6. As a user, I want a Source that cannot be measured (Claude Code records no
   Cost) to say so explicitly, so that a blank is never mistaken for "spent
   nothing".
7. As a user, I want each group row annotated with the change since the
   previous read, so that I can see what is *moving* rather than only the
   running total.
8. As a user, I want the first frame to show no delta rather than `+0`, so that
   the display never claims to know a change it has not yet observed.
9. As a user, I want `--calendar --last 1d` to show *today* on every reload, so
   that a watch left running across midnight rolls into the new day instead of
   silently showing yesterday's bucket.
10. As a user, I want `--calendar --last 1d` with a `1d` budget to measure
    exactly the same records, so that the window and the ceiling agree and no
    "clipped window" caveat appears.
11. As a user, I want `watch --interval 0` to be rejected with a clear error,
    so that I cannot accidentally ask for a busy-loop hammering my disk.
12. As a user, I want `watch --interval 1h` to keep counting down, so that a
    long interval simply polls more slowly and can still be read at a glance.
13. As a user, I want to press `r` to reload immediately, so that I do not have
    to wait out a long interval after finishing a big session.
14. As a user, I want `watch` to honor `--project`, `--model`, `--source`,
    `--since`, `--last`, `--group-by`, and the budget flags, so that a monitor
    can be pointed at exactly the slice I care about.
15. As a user, I want a source that fails to load to be shown as failed in the
    sources strip while `watch` keeps running, so that one broken source does
    not kill the monitor.
16. As a user, I want `watch --json` to dump exactly one frame and exit, so
    that a monitor can be sampled from a script without becoming a service.
17. As a maintainer, I want `watch` to reuse the existing window, aggregation,
    and budget functions rather than deriving any of them again, so that it
    cannot drift from `usage`/`report` on what a window or a budget means.
18. As a maintainer, I want `watch` to add no new output format, no new field
    set, and no new dependency, so that it is a view of data that already
    exists and not a new data model.

## Implementation Decisions

### CLI surface

- New `Command::Watch(WatchArgs)` in `src/cli.rs`.
- `WatchArgs` implements the three existing shared traits rather than copying
  their accessors: `FilterArgs`, `SourcePathArgs`, and `BudgetArgs`. It carries
  no `--json`/`--csv` pair; a single `--json` bool selects the one-shot dump.
- `WatchArgs` adds exactly three new fields: `--last`, `--calendar` (both
  already the shared `WindowArgs` vocabulary — `WatchArgs` implements
  `WindowArgs` too), and `--interval <seconds>` (u64, clap
  `default_value_t = 5`).
- Window validation is `WindowArgs::validate_window()` / `window_mode()`
  verbatim, so `watch --calendar --last 4h` fails with the *same* message
  `usage` produces. No new window logic exists.

### Rendering: `--json` reuses the `usage` renderer, byte for byte

`watch --json` prints the output of `OutputRenderer::json` with the same
arguments `run_usage` passes. It is not "a watch format" — it is a `usage`
frame, which makes the JSON contract testable by asserting equality with the
`usage --json` output for the same window.

`watch` deliberately does **not** gain `--csv`: a CSV has no place to put
"updated at", "next read", or per-row deltas, and inventing one would create a
second, drifting format. `usage --csv` remains the CSV surface.

### Delta tracking lives in the app, not the renderer

`WatchApp` holds `prev_agg: Option<AggregateResult>` and `last_updated`. On
each `apply_data` the *outgoing* aggregate becomes the baseline; when there is
no baseline (the first frame) the renderer draws `—`.

- `token_delta(key) -> Option<i64>` sums `(in+out+cache_read+cache_write)`
  across the whole `TokenBreakdown` — a group that shifted tokens between
  buckets is a change, not a no-op.
- `cost_delta(key) -> Option<f64>` is `Some` only when **both** frames report a
  cost for that key. A group that gained or lost its cost (e.g. it now includes
  a Claude record) renders `—` rather than a wrong number, honoring the
  source-scoped cost invariant from ADR 0001.
- Deltas are keyed by the group key **string**. Changing `--group-by` mid-run
  does not exist in `watch` (no `Tab`), so keys are stable for the life of the
  process.

### Countdown

`WatchApp` stores `next_refresh_at: Option<DateTime<Utc>>`, set to
`loaded_at + interval` on every apply. `countdown_text(now)` returns
`"next in 5s"`, `"next in 1m 5s"` beyond a minute, `"reloading…"` at or past
zero (the reload is already in flight — the timer thread ticks on schedule by
construction), and `""` when no deadline is known. The value is pure in `now`,
so it is unit-testable without a terminal or a sleeping thread.

### TUI plumbing

- `src/tui/watch_app.rs`: `WatchApp` + `WatchTuiState { app, table_state }`
  (a `TableState` for scroll offset only — `watch` has no selection) +
  `WatchTuiApp` (terminal, mirroring `TerminalApp`).
- `src/tui/watch_render.rs`: layout is
  `header(6) | sources(3) | table(min 8) | footer(3)`.
- `WatchApp` is `#[derive(Default)]`; `GroupBy` already defaults, and
  `prev_agg`/`next_refresh_at` are `Option`.

### Main wiring

`run_watch` mirrors `run_tui`'s shape, not its body: a
`tokio::sync::mpsc::channel(1)` fed by a `tokio::time::interval` task that
rebuilds the window from a fresh `now` per tick, plus an initial synchronous
load before the first draw. During a reload the UI stays responsive because
only one reload is ever in flight on the loader thread and the channel drops
stale frames.

One addition `usage` does not have: `TuiData` (the loader payload) gains an
`updated_at: DateTime<Utc>` field, set to the same `now` that built the window
and evaluated the budgets. The renderer must not read the clock to describe
data it did not measure — the same rule spec 0018 applied to the report header
and `DiffTuiData::loaded_at` already applies.

## User Interface

```
╭ llmhelper watch ───────────────────────────── updated 12:00:03  next in 5s ╮
│ active  12:00:00 → 12:00:03 (3s)          window: last 1d (calendar)       │
│ budget: 1 over                                                            │
╰───────────────────────────────────────────────────────────────────────────╯
╭ sources ──────────────────────────────────────────────────────────────────╮
│ ● claude 1284 records loaded   ⚠ opencode 39 records loaded               │
╰───────────────────────────────────────────────────────────────────────────╯
╭ usage by source ──────────────────────────────────────────────────────────╮
│ source   sessions  messages  tokens    Δtokens   cost       Δcost         │
│ claude         12        84   1.2M          —        —           —        │
│ ⚠ opencode       9        51   3.4M        +1.5K   3.870000    +0.120000  │
╰───────────────────────────────────────────────────────────────────────────╯
  r reload   q quit
```

- Guard when no budgets are configured: the `budget:` line is **omitted**, not
  rendered empty. A run with no budgets looks like `usage`'s header with two
  extra fields.
- Guard when a Source records no Cost: the cost cell is `—`, never `0.000000`.

## Error Handling

- `--interval 0` → exit 1,
  `invalid --interval 0: must be at least 1 second`. A zero interval would
  busy-loop the loader; it is a user error, not a silent clamp.
- `--interval` and the window combination are validated **before** any source
  is read, so a bad invocation fails instantly.
- A source that fails to load keeps its existing contract: the sources strip
  marks it `✗` in red with the error text, `load_all` contributes zero records
  for it, and the monitor keeps running.
- `watch` writes nothing to stdout in TUI mode, exactly like `usage`.

## Testing Decisions

- **`--json` is the CLI seam.** Integration tests run `watch --json` and
  `usage --json` with identical flags over the shared fixture tree and assert
  the outputs are **equal**. That single assertion proves the loader, the
  window, the aggregation, the grouping, and the serializer are the same code
  path; anything that diverges fails here.
- **Source-path isolation.** Every test passes all four override flags. Tests
  that assert an *empty* result point all four at a temp directory of
  nonexistent paths, so a developer's real `~/.claude/projects` can never leak
  into an assertion (the trap spec 0018 recorded).
- **Calendar fixture.** Relative Claude sessions built against
  `local_midnight_days_ago(0)` / `(1)`, reusing the helper spec 0017 built, so
  `--calendar --last 1d` selects today and excludes yesterday deterministically
  in any timezone.
- **State-level delta tests.** `token_delta`/`cost_delta`/`countdown_text` are
  pure functions of `WatchApp` state, so the interesting cases (no baseline →
  `None`; a group appearing or disappearing; a group that lost cost → `None`;
  countdown formatting and the `reloading…` boundary) are unit tests with no
  TTY, no thread, and no clock.
- **Rendering tests.** `header_line`/`footer_line` return `Line`s and are
  flattened to strings in tests, the pattern `request_render.rs` established —
  no `TestBackend`, no terminal.
- **PTY smoke.** `.scratch/watch_tui_smoke.py` (the existing convention, not
  checked in) drives the real binary through a PTY: assert the initial frame
  carries the title, `updated`, and `next in`, that `r` re-renders, that the
  countdown value changes between the first frame and a frame read a couple of
  seconds later, and that `q` exits 0.

## Out of Scope

- **No selection, detail view, or `Tab` group cycling.** `usage` owns
  interactive browsing; `watch` is a monitor. Reintroducing them here would
  duplicate `usage`'s state machine for no new information.
- **No historical sampling.** `watch` keeps no ring buffer of past frames and
  draws no sparkline; deltas are "now vs the previous read" and nothing more.
  Trend data is `diff`'s job (spec 0002/0017).
- **No `--csv`.** See the rendering decision above.
- **No growth alerts, sound, or desktop notifications.** A crossed budget is
  rendered loudly; alerting is a different feature with different failure modes
  (a monitor that can page you is a daemon, and this is a TUI).
- **No new sources, fields, or dependencies.** `watch` renders data that
  `usage` already computes.
- **No `--interval` unit suffixes.** The flag takes whole seconds; `--last`
  already owns the `<n><unit>` vocabulary and overloading it here would invite
  `--interval 1d`, which is meaningless for a refresh timer.

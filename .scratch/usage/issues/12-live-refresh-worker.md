# 12 — Live-refresh worker (tokio ticker → mpsc)

**What to build:** The background refresh loop from ADR-0003: a `tokio::spawn` task that, on a fixed interval (default several seconds, later overridable via config in 14), re-calls `registry.load_all()`, re-aggregates, and sends the fresh result over a **bounded** `mpsc::channel(1)` (drop stale batches if the TUI lags). The TUI (11) receives on this channel and updates its `AppState` + redraws. Full re-read each tick — no incremental cache.

**Blocked by:** 03, 06 (registry + discovery to load), 08 — re-aggregates each tick. 11 — TUI consumes the channel (can land in parallel with 11; integration at the TUI's `AppState`).

**Status:** ready-for-agent

- [ ] A spawned worker holds a ticker at `refresh_interval_seconds` and emits a fresh `(Vec<Record>, Vec<SourceStatus>)` over `mpsc::channel(1)` each tick
- [ ] Full re-read each tick (no mtime/size cache) per ADR-0003
- [ ] TUI's `AppState` updates from the latest channel batch and re-renders; stale batches beyond capacity are dropped, not queued
- [ ] `r` key (from 11) forces an immediate reload outside the interval
- [ ] Smoke test: leave the TUI open against a fixture, mutate a fixture, observe the screen update within one interval without crashing

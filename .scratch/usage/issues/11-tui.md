# 11 — TUI: ratatui single-screen (state + render + keys)

**What to build:** The default `usage` experience: a ratatui single screen with a grand-totals header and a table of groups (columns: group key, sessions, messages, input, output, reasoning, cache-read, cache-write, cost). `Tab` cycles grouping Source → Project → Model; `q`/`Esc` quit; `r` forces a refresh. The TUI reads from an `AppState` (the aggregator result) held in shared state and re-renders on each frame — it does not itself spawn the refresh worker (that's 12). Initial render must work from a single loaded snapshot.

**Blocked by:** 03 (source statuses for the status panel), 08 — renders aggregator output.

**Status:** ready-for-agent

- [ ] ratatui app boots into a full-screen layout: header (grand totals) + group table + source-status line
- [ ] `Tab` cycles `GroupBy` and re-renders the table grouped accordingly; `q`/`Esc` exit the loop; `r` triggers a refresh signal (consumed by 12)
- [ ] Table shows the nine columns; cost blank for Claude groups, numeric for OpenCode
- [ ] Source-status line shows each registered source's name + record count + status (ok/absent/unreadable)
- [ ] Manual smoke test: `llmhelper usage` against fixture overrides renders the screen and `Tab`/quit work

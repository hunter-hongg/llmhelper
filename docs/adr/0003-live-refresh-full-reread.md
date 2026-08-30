# Live auto-refresh re-reads all sources each tick

The TUI auto-refreshes by re-reading and re-parsing every Source on a fixed interval (default several seconds, configurable via TOML). Claude transcripts are re-parsed message-by-message; OpenCode is re-queried.

We chose full re-read over an incremental cache keyed on mtime/size. Full re-read is trivially correct with no stale-cache edge cases, and is fast enough at a modest interval given typical transcript sizes. The trade-off: on machines with many large Claude transcripts, CPU use between ticks is higher than an incremental approach. We explicitly defer the incremental optimization until profiling shows it is actually needed — do not add caching preemptively.

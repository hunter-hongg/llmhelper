# 10 — CSV output mode

**What to build:** `usage --csv` renders the same aggregator result as CSV: one header row then one row per group. Columns: `group_key, source, sessions, messages, input, output, reasoning, cache_read, cache_write, cost`. `cost` empty for Claude groups. Shares the aggregator output with 09; no separate data path.

**Blocked by:** 08 — renders aggregator output (no source-status block required, but may include a leading comment or omit it).

**Status:** ready-for-agent

- [ ] `--csv` flag dispatches to a CSV serializer; `--json` and `--csv` are mutually exclusive (validated at CLI in 13)
- [ ] Header row exactly `group_key,source,sessions,messages,input,output,reasoning,cache_read,cache_write,cost`
- [ ] One data row per group; `cost` blank when `None`, numeric when `Some`
- [ ] A test asserts CSV parses with the right column count and values for a known fixture set

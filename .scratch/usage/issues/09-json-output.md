# 09 — JSON output mode

**What to build:** `usage --json` renders the aggregator result (source statuses + group_by + groups) as a single JSON object and prints it, no TUI. This is also the seam the integration tests (15) drive. Schema: top-level `{ sources: [{name, records, status}], group_by, groups: [{key, source, sessions, messages, tokens:{...}, cost}] }`. Claude groups emit `cost: null`; OpenCode groups emit `cost: <number>`.

**Blocked by:** 03 (source statuses), 08 — renders aggregator output.

**Status:** ready-for-agent

- [ ] `--json` flag on the `usage` args dispatches to a JSON serializer instead of the TUI
- [ ] Output matches the spec schema: `sources` with per-source status, `group_by`, and `groups` each with key/sessions/messages/tokens/cost
- [ ] `cost` serialized as JSON `null` when `None`, number when `Some`
- [ ] A test runs the binary (or calls the renderer on fixtures) and asserts the JSON parses and matches expected values for a known fixture set

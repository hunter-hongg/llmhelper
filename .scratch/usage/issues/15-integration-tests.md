# 15 — Integration fixtures + single-seam assertions

**What to build:** The one testing seam from the spec: a fixture corpus (Claude JSONL directory with a `model:"auto"` session and a multi-message session; OpenCode SQLite with a duplicated session id across `opencode.db` + `opencode-local.db` so dedupe is exercised) plus integration tests that drive the real `usage --json` binary against these fixtures and assert external behavior. This is the terminal vertical slice — it only turns green once 04–14 all land. It proves adapter → normalization → discovery → filter → aggregate → output in one deterministic pass.

**Blocked by:** 04, 05, 06, 07, 08, 09, 13 — every layer the seam exercises must exist. (10 CSV and 11/12 TUI are not required for the JSON seam to pass, but they should land first or alongside; the seam targets `--json`.)

**Status:** ready-for-agent

- [ ] Fixtures under `tests/fixtures/` as specified: claude dir with 2 sessions (one `auto`), opencode db pair with one duplicated id
- [ ] A test asserts total session count = 3 after dedupe; per-source record counts correct
- [ ] A test asserts Claude `cost` is `null` everywhere; OpenCode `cost` is `Some`
- [ ] A test asserts token sums match fixture values; `--source claude` emits only Claude groups; `--last 7d` emits only recent; `--group-by model` yields the fixture model buckets
- [ ] The full seam passes from `cargo test` deterministically with no TUI, no network, no real home-dir access

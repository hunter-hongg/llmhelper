# 05 — OpenCode adapter (SQLite → Record)

**What to build:** An `OpenCodeSource` impl of `Source` that queries one or more OpenCode SQLite DBs (`opencode.db`, `-local`, `-dev`) and emits `Record`s from the `session` table. It maps `tokens_input/output/reasoning/cache_read/cache_write` → the canonical five fields, normalizes the `model` JSON object (`{"id":"x","providerID":"y"}` → `"x"`; raw string fallback if unparseable), reads `cost` as `Some(f64)`, and converts `time_created`/`time_updated` (Unix ms) to `DateTime<Utc>`. It merges all supplied DBs, deduplicating sessions by `id` (a session may appear in two DBs).

**Blocked by:** 03 — Source trait + registry (must impl `Source`).

**Status:** ready-for-agent

- [ ] `OpenCodeSource::new(dbs: Vec<PathBuf>)` constructs a source over one or more DB paths
- [ ] `SELECT` from `session` maps token columns → `TokenBreakdown`; `cost` → `Some(f64)`
- [ ] `model` JSON object normalized to its `"id"`; raw string on parse failure
- [ ] `time_created`/`time_updated` (ms) → `DateTime<Utc>` for `started_at`/`ended_at`
- [ ] Union of all supplied DBs deduplicated by session `id`
- [ ] A unit test feeds a fabricated SQLite with a duplicated session id across two DBs and asserts the record count after dedupe; another asserts `["id":"x"]` normalizes to `"x"`

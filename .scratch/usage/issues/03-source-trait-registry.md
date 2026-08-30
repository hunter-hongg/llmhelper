# 03 — Source trait + registry skeleton

**What to build:** The extension seam from ADR-0002: a `Source` trait with `fn load(&self) -> Result<Vec<Record>, SourceError>` and a registry (`BTreeMap<String, Box<dyn Source>>`) built at startup. `SourceError` carries a per-source status (absent / unreadable) so a failing source degrades gracefully (ADR implicit: degrade per-source, never crash). The registry is enumerable so the aggregator and UI see which sources contributed and how many records each produced — even when a source is absent.

**Blocked by:** 02 — Domain model (trait returns `Vec<Record>`).

**Status:** ready-for-agent

- [ ] `Source` trait defined with `load(&self) -> Result<Vec<Record>, SourceError>`
- [ ] `SourceError` enum/distinct variant captures `Absent` | `Unreadable(String)`; `load` returns `Ok(vec![])` with a recorded status on absence rather than `Err`
- [ ] A `Registry` (or builder function) holds `BTreeMap<String, Box<dyn Source>>` and exposes `load_all() -> (Vec<Record>, Vec<SourceStatus>)` where `SourceStatus = (name, record_count, Option<error>)`
- [ ] Registry is constructed empty + a `register(name, source)` entry point so adapters (04, 05) plug in without touching dispatch
- [ ] A unit test registers a fake source returning fixture records and asserts `load_all` returns them with correct status

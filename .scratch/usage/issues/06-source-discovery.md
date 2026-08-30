# 06 — Source discovery + override wiring

**What to build:** A discovery step that, at startup, finds the default locations for both sources (`~/.claude/projects` for Claude; the three `~/.local/share/opencode/*.db` files for OpenCode), constructs the matching adapter, and registers it. The discovery must accept overrides (a custom Claude dir, a custom list of OpenCode DB paths) so it composes cleanly with the later config-file and CLI-flag tickets — but the override input is just constructor args here; the flag/config plumbing lands in 13 and 14.

**Blocked by:** 04, 05 — both adapters exist to be discovered and registered.

**Status:** ready-for-agent

- [ ] A `discover_sources(claude_override: Option<PathBuf>, opencode_override: Option<Vec<PathBuf>>) -> Registry` builds the registry from defaults when overrides are `None`
- [ ] Default Claude path resolves from `$HOME/.claude/projects`; default OpenCode paths from `$HOME/.local/share/opencode/{opencode,opencode-local,opencode-dev}.db` (existing files only)
- [ ] Missing default directories/DBs degrade per-source (status `Absent`) rather than erroring
- [ ] A test with `claude_override` pointing at a fixture dir and `opencode_override` at a fixture DB produces a registry whose `load_all` returns the expected records

# 14 — Config file (XDG TOML)

**What to build:** Reads `~/.config/llmhelper/config.toml` (XDG-compliant, optional) supplying defaults: `[source.claude] dir`, `[source.opencode] db = [...]`, `[ui] refresh_interval_seconds`. Config values seed discovery (06) and the refresh interval (12) only when the corresponding CLI flag is absent — CLI always wins. File is optional; absence is not an error.

**Blocked by:** 06 (config seeds discovery overrides), 12 (config seeds refresh interval).

**Status:** ready-for-agent

- [ ] Config parsed with `toml`; missing file or unreadable file → defaults, no error
- [ ] `config.source.claude.dir` and `config.source.opencode.db` fed to discovery only when CLI `--claude-dir` / `--opencode-db` are unset
- [ ] `config.ui.refresh_interval_seconds` seeds the worker interval only when no CLI override exists
- [ ] A test with a fixture config + fixture paths asserts the resolved (config-or-flag) override precedence behaves correctly

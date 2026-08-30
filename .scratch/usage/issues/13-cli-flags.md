# 13 — CLI flags: filters, output mode, source overrides

**What to build:** The full `usage` argument surface wired to the layers built so far: `--since`, `--last` (mutually exclusive), `--project`, `--model`, `--source`, `--json`/`--csv` (mutually exclusive), plus `--claude-dir` and `--opencode-db` (repeatable) override knobs that feed discovery (06). Flags override any config (14). Invalid combos (`--since`+`--last`, `--json`+`--csv`) produce a clear Clap error before any loading.

**Blocked by:** 06 (discovery overrides), 07 (filters), 08, 09, 10 — all the layers the flags select.

**Status:** ready-for-agent

- [ ] `usage` args struct declares every flag above with correct Clap types (duration parser, repeatable `--opencode-db`, enum for `--source`/`--group-by`)
- [ ] `--claude-dir` / `--opencode-db` passed into discovery; `--since`/`--last`/`--project`/`--model`/`--source` built into the `Filter`
- [ ] `--json` / `--csv` select output mode; mutually-exclusive pairs rejected by Clap before execution
- [ ] `llmhelper usage --help` documents every flag; a CLI test exercises `--source claude` and `--last 7d` against fixtures and asserts the JSON output filters correctly

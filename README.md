# llmhelper

A Rust + Clap CLI for agent/LLM usage introspection. Reads local usage data from Claude Code, OpenCode, OMP and Kilo Code and renders it via ratatui TUI or Markdown reports.

## Subcommands

### usage
Interactive TUI aggregate view.
```bash
llmhelper usage --last 7d --group-by project
llmhelper usage --project myproj --model auto --json
```
Flags: `--since/--last`, `--project`, `--model`, `--source`, `--group-by source|project|model`, `--json/--csv`.

### diff
Sliding-window comparison between two periods.
```bash
llmhelper diff --last 7d --prev 7d --group-by project
llmhelper diff --last 30d --prev 30d --json
```

### sessions
List sessions with paging and detail.
```bash
llmhelper sessions --last 30d --limit 20
llmhelper sessions --detail <session_id>
llmhelper sessions --project myproj --json
```

### report
Shareable Markdown summary with an interactive viewer.
```bash
llmhelper report --last 7d --group-by project --top 10
llmhelper report --since 2026-08-01 --source claude --model auto
llmhelper report --last 7d --title "Team weekly LLM usage" --output weekly.md
```
Without `--output`, `report` opens an interactive TUI that renders the Markdown report: scroll with `↑↓`/`j k` or PgUp/PgDn, jump with `g`/`G` (top/bottom), quit with `q`. The header shows the scroll position (`lines X-Y of N`).

With `--output <path>` the report is written to the file instead (stdout stays empty), suitable for pasting into chat/PR/notes or committing. Header includes generation time, window and applied filters. Totals, per-source Cost, and a grouped usage table are rendered. `--top n` truncates the groups table with a `(+ k more …)` summary line; `--title <string>` replaces the default `# llmhelper report` heading.

Cost is source-scoped and never summed across sources.

## Sources

- claude – transcript JSONL
- opencode – SQLite
- omp – `~/.omp/agent/sessions`
- kilo – SQLite `kilo.db` under `~/.local/share/kilo`

Auto-discovered at defaults; override with `--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db` or config file.

## Build & Test
```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

## Specs
See `docs/specs/` for usage, diff, sessions and report specifications. Architecture notes in `docs/adr/`.

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

### request
Send an OpenAI-compatible Chat Completions request with TUI or CLI output.
```bash
# Interactive TUI viewer (default)
llmhelper request --base-url https://api.example.com --model gpt-4 --prompt "Hello"

# JSON output
llmhelper request --base-url https://api.example.com --api-key $KEY --model gpt-4 \
  --messages messages.json --json

# Plain text output (only assistant content)
llmhelper request --base-url https://api.example.com --model gpt-4 --prompt "Hi" --text
```

With `--stream` the response is consumed as SSE and deltas appear as they arrive instead of blocking until the full response. All three output modes have a streaming form:
```bash
# Streaming TUI (default) — header shows `stream: live`, then `stream: done`
llmhelper request --base-url https://api.example.com --model gpt-4 --prompt "Explain TCP" --stream

# Incremental text — each delta is printed and flushed as it arrives, pipable
llmhelper request --base-url https://api.example.com --model gpt-4 --prompt "Explain TCP" --text --stream

# NDJSON — one JSON object per SSE event, pipeable into other tools
llmhelper request --base-url https://api.example.com --model gpt-4 --messages messages.json --json --stream
```

With `--stream` the payload carries `stream: true` and `stream_options.include_usage`, so providers following the OpenAI schema report token counts in a stream chunk. Without `--stream` the one-shot payload carries neither key. A provider that rejects unknown request fields surfaces that as an HTTP error with the provider's body snippet; there is no way to detect it proactively.

Flags: `--base-url` (required unless in config), `--api-key` (env `LLMHELPER_API_KEY` fallback), `--model` (required), `--messages <path>` (JSON array of `{role, content}`), `--prompt <string>` (single user turn), `--json` prints full response, `--text` prints only assistant message content, `--stream` streams the response as SSE, `--temperature`, `--top-p`, `--max-tokens`, `--stop` (repeatable). Configuration via `[request]` section in `~/.config/llmhelper/config.toml`.

### search
Full-text search across agent session message text.
```bash
llmhelper search "cache invalidation"
llmhelper search --last 7d --source omp --role assistant "refactor"
```
Without `--json`/`--csv`/`--text`, `search` opens an interactive TUI: select a hit and press `Enter` for the message detail, `Esc` to return, `↑↓`/`j k` to move, `g`/`G` for top/bottom, `r` to rerun, `q` to quit. The header shows the query plus any non-default active filters.

Flags: `--source`, `--project`, `--model`, `--role`, `--since`/`--last`, `--context` (snippet context, default 80), `--limit` (default 100), `--case-sensitive`, `--json`/`--csv`/`--text`. Messages without a timestamp never match a time filter. Tool output and image/patch blocks are excluded from the searchable corpus.

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
See `docs/specs/` for the usage, diff, sessions, usage-agent-detail, report, report-output-title, report-tui, request, request-stream and search specifications. Architecture notes in `docs/adr/`.

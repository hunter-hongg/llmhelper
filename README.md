# llmhelper

A Rust + Clap CLI for agent/LLM usage introspection. Reads local usage data from Claude Code, OpenCode, OMP and Kilo Code and renders it via ratatui TUI, Markdown reports, or machine-readable exports.

## Subcommands

### usage
Interactive TUI aggregate view.
```bash
llmhelper usage --last 7d --group-by project
llmhelper usage --project myproj --model auto --json
```
Flags: `--since/--last`, `--project`, `--model`, `--source`, `--group-by source|project|model`, `--json/--csv`, `--budget <source:amount>` (repeatable), `--budget-window <spec>`, `--budget-name <name>` (repeatable).

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

`report` and `usage` also accept the budget flags `--budget`, `--budget-window`, and `--budget-name` (see [Budgets](#budgets)). When budgets are configured, `report` gains a `## Budget` section and `usage` flags over-budget Source rows.

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

#### Reasoning capture

A reasoning model answers behind a chain-of-thought that lands in a response field the command does not read by default. `--reasoning-field` names that field as a dotted path, and `--thinking` surfaces the result: it expands the reasoning pane in the TUI, and prints the reasoning alone under `--text`. Both are opt-in: a run without them is byte-for-byte unchanged.

```bash
# One-shot: read the chain-of-thought from a provider-specific field
llmhelper request --base-url https://api.example.com --model gpt-4 --prompt "Explain TCP" \
  --reasoning-field choices.0.message.reasoning --text --thinking

# Streaming: `choices.0.delta.reasoning_content` arrives alongside the answer deltas
llmhelper request --base-url https://api.example.com --model gpt-4 --prompt "Explain TCP" \
  --reasoning-field choices.0.delta.reasoning_content --text --stream
```

`--reasoning-field` is repeatable; every named field is consulted and the non-empty ones are joined in flag order. A dotted path walks nested objects and array indices (`choices.0.message.reasoning`), and a missing path contributes nothing rather than erroring. `--thinking` selects the view that shows the reasoning pane at launch; inside the TUI, `t` cycles the view between answer-only, both, and thinking-only, and the header reports whether the generation is `thinking`, `answering`, or `done`.

`--reasoning <file>` is a separate concern: it passes a request-side configuration object (effort, budget, …) through to the provider. The file must contain a JSON object, which is embedded verbatim as the payload's `reasoning` key — the command does not interpret it.

```json
{ "effort": "high" }
```

The output contract per capture path:

| Invocation | Output |
|---|---|
| `--json` | Wraps the provider response in an envelope: `{"response": <raw>, "reasoning": <joined text>, "reasoning_fields": [...]}`. Without capture the raw provider object is printed alone, as before. |
| `--text` | Appends the reasoning after the answer under a `[reasoning]` heading. |
| `--json --stream` | One SSE event per line with a sibling `"@channel"` key (`"content"` or `"reasoning"`) in a fixed position, so a consumer reading the raw keys is unaffected. |
| `--text --stream` | Answer deltas go to stdout, reasoning deltas to stderr. |
| `--text --thinking` / `--text --thinking --stream` | Prints the reasoning alone (to stdout under `--stream`). |

In the TUI the reasoning renders in its own `thinking` pane with an independent scroll offset, and `--copy` copies whichever channel the current view shows. A run without reasoning keeps today's two-pane layout exactly.

Configuration via `[request]` section in `~/.config/llmhelper/config.toml`:

```toml
[request]
reasoning_fields = ["choices.0.message.reasoning"]
reasoning = "reasoning.json"   # relative paths resolve against the config file's directory
```

Both keys are fallbacks: a CLI `--reasoning-field`/`--reasoning` overrides them. Relative paths in the config resolve against the config file's own directory so a config and the files it references move together; absolute paths are left alone.

Flags: `--base-url` (required unless in config), `--api-key` (env `LLMHELPER_API_KEY` fallback), `--model` (required), `--messages <path>` (JSON array of `{role, content}`), `--prompt <string>` (single user turn), `--json` prints full response, `--text` prints only assistant message content, `--stream` streams the response as SSE, `--reasoning-field <path>` (repeatable) captures reasoning text from a response field, `--reasoning <file>` passes a reasoning configuration object through to the provider, `--thinking` expands the reasoning view in the TUI, `--temperature`, `--top-p`, `--max-tokens`, `--stop` (repeatable). Configuration via `[request]` section in `~/.config/llmhelper/config.toml`.

### search
Full-text search across agent session message text.
```bash
llmhelper search "cache invalidation"
llmhelper search --last 7d --source omp --role assistant "refactor"
```
Without `--json`/`--csv`/`--text`, `search` opens an interactive TUI: select a hit and press `Enter` for the message detail, `Esc` to return, `↑↓`/`j k` to move, `g`/`G` for top/bottom, `r` to rerun, `q` to quit. The header shows the query plus any non-default active filters.

Flags: `--source`, `--project`, `--model`, `--role`, `--since`/`--last`, `--context` (snippet context, default 80), `--limit` (default 100), `--case-sensitive`, `--json`/`--csv`/`--text`. Messages without a timestamp never match a time filter. Tool output and image/patch blocks are excluded from the searchable corpus.

### export
Flat, machine-oriented dump of the filtered Session records — one row per
Session, no aggregation. The program-facing counterpart to `report`.
```bash
llmhelper export --last 30d                     # one JSON object per line (default)
llmhelper export --format json                  # a single JSON array
llmhelper export --format csv --fields source,project,model,input,output,cost
llmhelper export --source omp --format tsv      # tab-separated
```
`--format` selects `jsonl` (default), `json`, `csv`, or `tsv`. JSONL is the
default because export's consumer is always a pipe or a file; a zero-match
filter exits 0 and writes nothing, so a scoped export that matches nothing is
not an error.

`--fields` picks the columns and their order (comma-separated and/or repeated);
with no `--fields`, every field is emitted in canonical order:
`source`, `session_id`, `project`, `model`, `agent`, `started_at`, `ended_at`,
`messages`, `input`, `output`, `cache_read`, `cache_write`, `cost`. An unknown
field name exits 1 and lists the valid names.

Rows are ordered by `started_at` descending, tie-broken by `(source,
session_id)`, so two exports of identical data are byte-identical. Token cells
are raw integers (not the `K/M/B` form) so a spreadsheet sees numbers; cost is
per-Record and never summed, and is `null` / an empty cell for Sources that
record no spend (Claude Code). Source-load errors print a `warn:` line on
stderr and never abort the export.

#### Exporting message text

`--messages` flips the unit of export from Session to transcript message — the
same corpus [`search`](#search) reads, but with no query, so you get the whole
thing. One row per message, in the same `--format`s, with `--fields` resolving
against the **message** field set instead of the session one.
```bash
llmhelper export --messages --last 7d --role assistant   # every assistant reply this week
llmhelper export --messages --fields session_id,role,text --format jsonl
llmhelper export --messages --source claude --format csv
```
The message fields, in canonical order: `source`, `session_id`, `project`,
`model`, `role`, `timestamp`, `text`. `role` is a new filter that narrows to one
role (e.g. `user`, `assistant`, `thinking`); it requires `--messages` and is a
case-insensitive exact match.

Rows are grouped by `(source, session_id)` ascending and, within a session,
ordered chronologically with a timestamp-less message sorted last — messages
read as a transcript, so they come out in the order they happened. `model` and
`timestamp` are `null` / an empty cell when the Source did not record them.
Time filters fail closed: a message with no timestamp is excluded by
`--since`/`--last` rather than admitted. `--fields` with a session-only name
(e.g. `cost`) exits 1 naming it as invalid for messages, and vice versa.

Tool-result and tool-use rows are excluded from the corpus, matching `search`.
As with session export, a zero-match filter exits 0 (`[]` for JSON, a header
row for CSV/TSV, nothing for JSONL), and Source errors warn on stderr without
aborting.

Flags: `--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db`, `--since`/`--last`, `--project`, `--model`, `--source`, `--format jsonl|json|csv|tsv`, `--fields`, `--messages`, `--role`.

## Sources

- claude – transcript JSONL
- opencode – SQLite
- omp – `~/.omp/agent/sessions`
- kilo – SQLite `kilo.db` under `~/.local/share/kilo`

Auto-discovered at defaults; override with `--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db` or config file.

## Budgets

A budget is a spend ceiling for exactly one Source. It answers "has this Source
cost more than I allowed, over this window?" — and nothing else. Budgets are
**annotation only**: they change what the report and `usage` TUI display, never
the exit code, so they are safe to leave configured.

Because Cost is always source-scoped (a group of records from mixed Sources has
no single meaningful Cost), a budget binds to one Source and is never summed
across Sources. Claude Code records no Cost at all, so a budget on it reports
`not measured` rather than a misleading `ok`.

Budget flags are accepted by `report` and `usage`. The annotation appears in the
report Markdown (including `report --output`) and in the `usage` TUI;
`usage --json`/`--csv` still validate the flags but emit no budget data, since
those are machine formats with a fixed shape.

### Declaring a budget

In `~/.config/llmhelper/config.toml`:

```toml
[budget.opencode-daily]
source   = "opencode"
window   = "1d"
max_cost = 5.00
```

`window` accepts a calendar or a free duration: `1d`, `7d`, `30d`, `1w`,
`1mo`, `12h`, `30m`. A calendar window (`1d`, `1w`, `1mo`) is anchored at local
midnight and never counts across that boundary: `1d` is today, `1w` the last 7
local days, `1mo` the last 30 local days (trailing windows, not ISO weeks or
calendar months). A free duration (`12h`, `30m`) is a rolling window from the
current instant.

### One-off budgets on the command line

`--budget <source>:<amount>` declares a budget without editing config, and is
repeatable. `--budget-window` sets its window (default `1d`).
`--budget-name <name>` restricts evaluation to configured budgets by name (all
configured budgets are used when no name is given).

```bash
# A $5/day ceiling on OpenCode, alongside any configured budgets
llmhelper report --last 7d --budget opencode:5.00

# A $20 rolling 12h ceiling on omp
llmhelper usage --budget omp:20 --budget-window 12h

# Only the configured budget named "opencode-daily"
llmhelper report --budget-name opencode-daily
```

### Reading the status

| status | meaning |
|---|---|
| `over` | the Source records Cost, and spend is **strictly greater** than `max_cost` |
| `ok` | the Source records Cost, and spend is at or below `max_cost` |
| `not measured` | the Source records no Cost (Claude Code), or contributed no records |

The boundary is strictly greater on purpose: spend exactly equal to the ceiling
has not crossed it.

A budget's window is its own property, not the command's `--last` window. When
the command loads a narrower range than the budget declares (a `30d` budget
inside a `--last 7d` run), the `## Budget` section says so and names the bound,
because the spend can then only be measured over the data that was loaded.

## Build & Test
```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

## Specs
See `docs/specs/` for the usage, diff, sessions, usage-agent-detail, report, report-output-title, report-tui, request, request-stream, request-reasoning, search, export, budget and message-export specifications. Architecture notes in `docs/adr/`.

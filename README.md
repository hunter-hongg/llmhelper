# llmhelper

A Rust + Clap CLI for agent/LLM usage introspection. Reads local usage data from Claude Code, OpenCode, OMP and Kilo Code and renders it via ratatui TUI, Markdown reports, or machine-readable exports.

## Subcommands

### Empty results and `--explain`
Every read command applies the same four predicate layers in the same order —
the time window (`--since`/`--last`/`--calendar`), `--project`, `--model`,
`--source`. When a combination matches nothing, the command says *why* instead
of printing an empty table:

```bash
$ llmhelper usage --project /typo-here --csv
group_key,source,sessions,messages,input,output,cache_read,cache_write,cost
no records matched — loaded 292, excluded by --project "/typo-here"
  filters: loaded 292 · window: — · project ("/typo-here"): 0 left · model: — · source: —
```

The second line is the **funnel**: how many records survived each layer, applied
cumulatively. Inactive layers show `—` rather than a carried-forward count, so
you can see at a glance which predicates were even in play. The window layer is
described by its resolved absolute bounds, never the keyword you typed.

*"No data at all"* is reported distinctly from *"filtered to nothing"*: when
nothing was loaded, the message is `no records loaded from any source` and **no
filter is blamed**, however many predicates you set.

`--explain` prints the funnel even on a non-empty result, which answers "how
much did `--project` cost me?" without first emptying it:

```bash
$ llmhelper usage --source claude --explain --csv
group_key,source,...
claude,claude,2,4,...
  filters: loaded 292 · window: — · project: — · model: — · source ("claude"): 22 left
```

Where it goes depends on the output mode:

| mode | where the diagnostic appears |
|---|---|
| table / TUI | stderr, so stdout stays clean for piping |
| `--csv` | stderr; the body stays a bare header, so the file still parses as CSV |
| `--json` | a structured `diagnostics` object inside the payload |

`--json` carries it as data, present only when the result is empty or
`--explain` was passed — so an existing non-empty run is unchanged:

```json
"diagnostics": {
  "loaded": 292,
  "stages": [
    {"layer": "window", "value": null, "remaining": 292},
    {"layer": "project", "value": "/typo-here", "remaining": 0},
    {"layer": "model", "value": null, "remaining": 0},
    {"layer": "source", "value": null, "remaining": 0}
  ],
  "matched": 0,
  "blamed": "project"
}
```

`sources[].records` is a **loaded** count, not a matched one, so an empty result
would otherwise show a populated `sources` panel beside `groups: []`. When
`diagnostics` is present, each source gains a `matched` count on the same row
— how many of its records survived the filter — so a consumer reading only
`sources` can no longer mistake a filtered-out corpus for usage. The field is
emitted only alongside `diagnostics`: an ordinary non-empty run is
byte-identical to before, and `sum(sources[].matched) == diagnostics.matched`.
`diff --json` carries no `matched`, since it has no funnel.

`report --output` embeds the funnel in the generated Markdown (there is no
terminal to print it to), under a `## Filters` section that appears only when
there is something to explain or `--explain` was passed. `search` counts
messages rather than records, so it reports its own message-level counts.

**Exit code stays `0`.** Narrowing to nothing is a legitimate result, not an
error, and turning it into a failure would break every existing script.

### usage
Interactive TUI aggregate view.
```bash
llmhelper usage --last 7d --group-by project
llmhelper usage --project myproj --model auto --json
```
Flags: `--since/--last`, `--project`, `--model`, `--source`, `--group-by source|project|model`, `--json/--csv`, `--explain`, `--budget <source:amount>` (repeatable), `--budget-window <spec>`, `--budget-name <name>` (repeatable).

With `--calendar`, `--last` snaps to a local-calendar bucket instead of a rolling duration, and takes calendar keywords only — `1d` (today), `1w` (the trailing 7 local days), `1mo` (30 days). A rolling duration such as `--last 4h` is rejected in calendar mode.
```bash
llmhelper usage --calendar --last 1d --group-by source
llmhelper usage --calendar --last 1w --json
```
Because the bucket is anchored at local midnight, the live TUI keeps showing *today* as the clock advances: each refresh re-anchors to the new day rather than sliding the window forward. `--calendar --last 1d` measures exactly the records a `1d` budget evaluates, so a matching budget is never flagged as clipped (see [Budgets](#budgets)).

### diff
Sliding-window comparison between two periods.
```bash
llmhelper diff --last 7d --prev 7d --group-by project
llmhelper diff --last 30d --prev 30d --json
```

By default both windows are rolling durations relative to now: current `[now-last, now]` and previous `[now-last-prev, now-last]`.

#### Calendar-aligned windows
Pass `--calendar` to snap both windows to local calendar boundaries instead. `--last`/`--prev` then take calendar keywords only — `1d` (today), `1w` (the trailing 7 local days), `1mo` (30 days) — and the comparison becomes *today vs the adjacent bucket before it*, anchored at local midnight rather than to the current clock time.
```bash
# Today vs yesterday (local midnight to now, and the full day before)
llmhelper diff --calendar --last 1d --prev 1d --group-by project

# Today vs the previous 7 days
llmhelper diff --calendar --last 1d --prev 1w --json

# This week so far vs the week before
llmhelper diff --calendar --last 1w --prev 1w
```
The two windows are always adjacent and non-overlapping: the previous window ends exactly where the current one begins. Because the current bucket is partial (local midnight → now), the header shows the keyword (`1d`) rather than a computed duration. A rolling duration such as `--last 4h` is rejected in calendar mode with a clear error; omit `--calendar` for rolling windows.

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
llmhelper report --calendar --last 1d --output today.md
```
Without `--output`, `report` opens an interactive TUI that renders the Markdown report: scroll with `↑↓`/`j k` or PgUp/PgDn, jump with `g`/`G` (top/bottom), quit with `q`. The header shows the scroll position (`lines X-Y of N`).

With `--output <path>` the report is written to the file instead (stdout stays empty), suitable for pasting into chat/PR/notes or committing. Header includes generation time, window and applied filters. Totals, per-source Cost, and a grouped usage table are rendered. `--top n` truncates the groups table with a `(+ k more …)` summary line; `--title <string>` replaces the default `# llmhelper report` heading.

As with `usage`, `--calendar` snaps `--last` to a local-calendar bucket (`1d`/`1w`/`1mo`) anchored at local midnight; the generated header marks the window as `last <kw> (calendar, local midnight)`.

Cost is source-scoped and never summed across sources.

`report`, `usage`, and `compare` also accept the budget flags `--budget`, `--budget-window`, and `--budget-name` (see [Budgets](#budgets)). When budgets are configured, `report` gains a `## Budget` section, `usage` flags over-budget Source rows, and `compare` marks the ranked row of an over-budget Source.

### watch
A live, self-refreshing usage monitor.
```bash
# Reload every 5 seconds (the default), showing the last 30 days
llmhelper watch --last 30d

# Today only, refreshing every 10s, with an over-budget warning
llmhelper watch --calendar --last 1d --interval 10 --budget kilo:5.00 --budget-window 1d

# One frame, exactly as `usage --json` — for scripts and sampling
llmhelper watch --last 7d --json
```

`watch` reads the same data, applies the same window and the same filters as `usage`, then keeps re-reading it on a timer instead of exiting. It answers "what is happening right now" rather than "what happened in this period": each refresh records a new frame, and every row is annotated with how much it moved since the previous one.

The header carries the freshness of the data — the clock time it was read at, a countdown to the next read, and how long the deltas span. On the first frame there is nothing to compare against, so the span reads `active — (first frame)` and every delta is `—` rather than a fabricated `+0`. A delta is shown only when both frames measured the value; a source that reports no cost in either frame gets a `—` in `Δcost`, never a `0` that would claim it was free.

Deltas are annotated `+`/`-` and coloured by sign (up green, down red). When `--budget`/`--budget-window` are given, an over-budget source is flagged in the sources strip and summarised in the header.

Keys: `r` reload immediately, `q` or `Esc` quit. That is deliberately the whole set — `watch` is a monitor, not a browser, so there is no row selection, no detail view and no `Tab` group cycling. Use `usage`'s TUI for those.

`--interval <seconds>` sets the reload period (default `5`; `0` is rejected). `--json` writes exactly one frame — byte-for-byte the same output as `usage --json` for the same flags — and exits, so a sampling script needs no second code path. There is no `--csv` for `watch`: a CSV has nowhere to record when the frame was read, when the next read is due, or what the deltas are measured against.

### trend
Usage split into aligned, whole time buckets — the "how has this changed over time" view that `usage` (a single window) cannot give.
```bash
# The last 30 days, one row per day
llmhelper trend --last 30d --bucket 1d

# The last 12 weeks, one row per week
llmhelper trend --last 12w --bucket 1w

# Weekly buckets, narrowed to one project, as JSON
llmhelper trend --last 90d --bucket 1w --project myproj --json
```

`trend` reads the same data and applies the same filters as `usage`, then places each surviving record into the bucket whose local-calendar range contains it. Every bucket is emitted, oldest first, including empty ones, so a gap in activity is a visible row of zeros rather than a missing line.

Both flags are required. `--bucket` takes only the calendar keywords `1d`, `1w` and `1mo`: a duration like `4h` has no local-day alignment and is rejected rather than silently producing a misaligned grid. `--last` is rounded up to whole buckets, so `--last 30d --bucket 1w` emits five weekly rows — the series may over-cover the window, never under-cover it. The header names the resolved span (first bucket's start → last bucket's end), not the flags you passed.

Each row is one bucket's totals; there is no per-group breakdown, so `trend` has no `--group-by`. To trend a single project, model or source, narrow the input instead — `--project`, `--model` and `--source` all filter *before* bucketing.

A bucket's `cost` is `—` when no source in it recorded a cost, and `0.000000` only when a source genuinely reported zero — a blank would be read as "spent nothing". Cost is never summed across sources: a bucket is priced only when all its records share one source and report a cost (ADR 0001). In JSON the same distinction is `null` versus `0.0`. The final bucket is open: it starts on a local midnight and ends at the current time, so it is the only row whose width can be a partial day.

`--explain` attaches the same filter funnel every other read command uses; on `--csv` the funnel goes to stderr so the body stays a bare header plus one row per bucket.

### compare
Rank the groups of one window against each other — the "who is eating my budget" view. `usage` prints a grouped table in map order with no share; `diff` contrasts the *same* group across two windows; `compare` orders the groups of a *single* window by one metric and shows each one's share of the whole.
```bash
# The biggest projects of the last 30 days, largest first
llmhelper compare --last 30d --group-by project

# Ranked by spend, top 7 with the tail folded into (others)
llmhelper compare --last 7d --group-by model --sort-by cost --top 7

# Ranked sources as JSON
llmhelper compare --last 30d --group-by source --json

# Rank sources and flag the one over its budget
llmhelper compare --last 30d --group-by source --budget opencode:5.00 --budget-window 1mo
```

`--sort-by` picks the ranking metric: `tokens` (default), `cost`, `sessions`, or `messages`. Rows are ordered descending, ties broken by group key, so the same data always produces the same bytes. Each row shows the metric's share of the total; the `share` column follows the sort metric, while the `token_shares` breakdown in JSON is always by token count.

`--top N` keeps the N largest groups and folds the rest into a single `(others)` row that still carries their summed share, so the visible percentages add up to 100. `--top 0` means no limit, and a limit at or above the group count folds nothing.

Cost obeys ADR 0001: a group is priced only when every contributing record shares one source and reports a cost. A cost-less group (Claude Code, or a group mixing sources) shows `—` / `null` / an empty CSV cell — never `0` — and is excluded from the cost-share denominator. When sorting by cost, cost-less groups sort last.

`compare` reuses `usage`'s filter and aggregation verbatim, so a group's totals can never differ between the two commands. `--explain` attaches the same funnel; on `--csv` it goes to stderr so the body stays a bare header plus one row per group. An empty result is a valid, header-only leaderboard and exits 0.

`compare` also accepts the budget flags `--budget`, `--budget-window`, and `--budget-name` (see [Budgets](#budgets)). When budgets are configured, a ranked row whose group belongs to an over-budget Source is marked with `⚠`, and the header reads `budgets: N over` (or `ok` / `not measured`). Because a budget is scoped to one Source (ADR 0001), only a row whose group is a *single* Source can be marked: a project or model group that spans Sources is `mixed` and is never attributed one Source's verdict. A Source that records no cost is left unmarked rather than shown as `ok`. In JSON each row gains a `budget_state` field (`over`/`under`/`not_measured`, absent when no budget applies) and the payload gains a `budgets` object; in CSV a `budget_state` column is appended. With no budgets configured, the output is byte-for-byte identical to a run without this feature, and the exit code never changes on an over budget — a budget is an annotation, not a gate.

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

#### Budget gate: the cost-control loop

`request` accepts the same budget flags as the reporting commands — `--budget <source>:<amount>` (repeatable), `--budget-window` (default `1d`), `--budget-name <name>` (repeatable) — and acts on them. Before any payload is built and before any provider is contacted, the named budgets are evaluated against the loaded Sources (only those the budgets name are read); if any is `over`, the request is refused:

```bash
# refuse if OpenCode already spent $5 today
llmhelper request --budget opencode:5.00 --prompt "hi" --text
#   error: budget gate: 1 budget is already over its ceiling:
#     cli:opencode (source opencode, window 1d): measured 12.500000 over ceiling 5.000000
#   refusing to send the request; no provider was contacted

# gate on my own prior llmhelper requests (see the loop below)
llmhelper request --budget-name mine --prompt "hi" --text
```

A refusal exits **3** — distinct from the usage errors (1: bad flags, a provider's non-2xx or unparsable answer) and transport errors (2: connection, DNS, timeout) that `request` already distinguishes — so a script can tell "refused by budget" from "provider failed". The gate is opt-in: with no budget flag given, `request` is byte-for-byte unchanged. Unlike the reporting commands, `--budget-name` with no names adopts **no** configured budgets (a gate must not silently inherit historical budgets); configured budgets gate `request` only when named. The gate evaluates once per invocation — follow-up `--interactive` turns are not re-gated.

The gate can also budget `llmhelper`'s own spending, closing the loop **measure → gate → spend → log → re-measure**:

```toml
# ~/.config/llmhelper/config.toml
[request]
log_dir = "logs"            # where --log writes; default ~/.config/llmhelper/logs

[price.gpt-4]               # per-million-token rates, keyed by the exact model
input_per_mtoken = 2.50     #   the provider echoes (quote keys containing dots)
output_per_mtoken = 10.00
# cache_read_per_mtoken / cache_write_per_mtoken are optional;
# both default to input_per_mtoken

[budget.mine]
source = "llmhelper"
window = "1d"
max_cost = 1.00
```

`llmhelper request --log --budget-name mine ...` records each turn's tokens to the per-day log; the `llmhelper` Source reads that log back, prices each turn with `[price.<model>]`, and the next invocation's gate refuses on the accumulated spend. `--log` is what feeds the loop: a budget naming `llmhelper` while `--log` is off prints a stderr warning (the gate cannot see unlogged requests) and proceeds. A model with no `[price]` entry reports no cost, and a budget on it is `not measured` — absent pricing is not free. Malformed or truncated log lines are skipped with a per-file warning, never fatal.

### search
Full-text search across agent session message text.
```bash
llmhelper search "cache invalidation"
llmhelper search --last 7d --source omp --role assistant "refactor"
```
Without `--json`/`--csv`/`--text`, `search` opens an interactive TUI: select a hit and press `Enter` for the message detail, `Esc` to return, `↑↓`/`j k` to move, `g`/`G` for top/bottom, `r` to rerun, `q` to quit. The header shows the query plus any non-default active filters.

Flags: `--source`, `--project`, `--model`, `--role`, `--since`/`--last`, `--context` (snippet context, default 80), `--limit` (default 100), `--case-sensitive`, `--json`/`--csv`/`--text`, `--explain`. Messages without a timestamp never match a time filter. Tool output and image/patch blocks are excluded from the searchable corpus.

#### Message cache

`search` (and `export --messages` — they share the corpus) memoizes message
extraction **per source file**: unchanged files are served from an index under
the platform cache directory instead of being re-parsed on every run. This is a
pure accelerator — a cached run and a `--no-cache` run produce byte-identical
non-interactive output. Search cache statistics appear under `--explain`: a
`cache` object in JSON, one line on stderr otherwise. The interactive search
TUI additionally shows reuse counts in its header without `--explain`.

```bash
llmhelper search --explain "query"        # show how much was reused
llmhelper search --json --explain "query" # …as a `cache` object in the payload
llmhelper search --no-cache "query"       # force a full re-extraction
llmhelper search --refresh-cache "query"  # discard the index, rebuild it
```

- The index lives at `~/.cache/llmhelper/` by default; `[cache] dir` in
  `config.toml` relocates it, `[cache] enabled = false` disables it.
- A file is re-extracted when its size, mtime, or inode changes; an appending
  transcript is therefore picked up on the next run. A corrupted index
  self-heals by re-extracting the affected files.
- `--refresh-cache` forces a rebuild; the two flags are mutually exclusive.
  `export --no-cache`/`--refresh-cache` only apply to `--messages`.

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

Flags: `--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db`, `--since`/`--last`, `--project`, `--model`, `--source`, `--format jsonl|json|csv|tsv`, `--fields`, `--messages`, `--role`, `--no-cache`, `--refresh-cache` (the latter two for `--messages`).

## Sources

- claude – transcript JSONL
- opencode – SQLite
- omp – `~/.omp/agent/sessions`
- kilo – SQLite `kilo.db` under `~/.local/share/kilo`
- llmhelper – the per-day request logs written by `request --log`, read back as
  usage: one record per logged response carrying a `usage` block, priced by the
  `[price.<model>]` config table (see [Budget gate](#budget-gate-the-cost-control-loop))

Auto-discovered at defaults; override with `--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db` or config file. The `llmhelper` Source appears only once its log directory exists — a user who never ran `request --log` sees no phantom row.

## Budgets

A budget is a spend ceiling for exactly one Source. It answers "has this Source
cost more than I allowed, over this window?" — and nothing else. On the
reporting commands budgets are **annotation only**: they change what the report
and `usage` TUI display, never the exit code, so they are safe to leave
configured. On `request` a budget is a **gate**: an over budget refuses the
request with exit code 3 before any provider is contacted (see
[Budget gate](#budget-gate-the-cost-control-loop)).

Because Cost is always source-scoped (a group of records from mixed Sources has
no single meaningful Cost), a budget binds to one Source and is never summed
across Sources. Claude Code records no Cost at all, and a model with no
`[price]` entry is unpriced, so a budget on either reports `not measured`
rather than a misleading `ok`.

Budget flags are accepted by `report`, `usage`, `compare` — and, as a gate, by
`request`. The annotation appears in the
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
| `not measured` | the Source records no Cost (Claude Code, or an unpriced `llmhelper` model), or contributed no records |

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
See `docs/specs/` for the usage, diff, sessions, usage-agent-detail, report, report-output-title, report-tui, request, request-stream, request-reasoning, search, export, budget, message-export, diff-calendar, usage-report-calendar, watch, empty-diagnostics, per-source-matched, trend, compare and compare-budget specifications. Architecture notes in `docs/adr/`.

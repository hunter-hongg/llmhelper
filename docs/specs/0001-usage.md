---
id: 0001
title: "usage — Agent/LLM usage introspection"
status: done
created: 2026-08-30
triage: done
---

## Problem Statement

There is no single tool that surfaces a unified view of token consumption and activity across the three agents the user runs locally: **Claude Code**, **OpenCode**, and **OMP**. Each stores usage data in a different format (Claude: per-message token blocks in JSONL transcripts; OpenCode: pre-aggregated rows in SQLite with a cost field; OMP: per-session JSONL under `~/.omp/agent/sessions` with per-message `usage` blocks including a cost object). The user wants a `usage` subcommand on a local CLI that reads all three sources, normalizes them, and presents aggregate usage in a terminal UI — without sending data anywhere.

## Solution

A Rust CLI (`llmhelper`) with a first subcommand `usage` that:

1. Auto-discovers Claude Code transcript JSONLs, OpenCode SQLite DBs, and OMP session JSONLs at their default local paths.
2. Loads and normalizes every session into a unified `Record` (session id, source, project, model, agent, timestamps, five-part token breakdown, message count, optional cost).
3. Exposes the data as a live-updating ratatui TUI: a grand-total header and a keyboard-toggleable table grouped by **Source / Project / Model**. The user can filter by time window, project path, model, and source via flags.
4. Also emits `--json` and `--csv` for scripting and pipeline use.

## User Stories

1. As an agent user, I want to see my total token consumption across all sessions so that I understand my usage footprint.
2. As an agent user, I want to see token breakdown (input / output / reasoning / cache-read / cache-write) per session so that I can understand where tokens are spent.
3. As an agent user, I want to see session and message counts per time window so that I know how actively I've used the agents.
4. As an agent user, I want to see per-Source cost where it exists (OpenCode) so that I understand spend without having to open the DB directly.
5. As an agent user, I want to group the table by Source, Project, or Model so that I can answer "how much did I spend in this repo?" and "which model am I using most?".
6. As an agent user, I want a live-updating TUI so that I see new sessions appear without re-running the command.
7. As a script author, I want `--json` output so that I can pipe `usage` into other tools (jq, dataviz, spreadsheets).
8. As a script author, I want `--csv` output so that I can import usage data into spreadsheet software.
9. As a power user, I want `--since` and `--last` flags so that I can answer "what did I use in the last 7 days?".
10. As a power user, I want `--project`, `--model`, and `--source` filter flags so that I can drill into specific subsets.
11. As a multi-machine user, I want `--claude-dir`, `--opencode-db`, and `--omp-dir` overrides so that `usage` works on any machine with non-default paths.
12. As a config-driven user, I want a `~/.config/llmhelper/config.toml` that sets defaults so that I don't repeat common flags.
13. As a user on a machine without OpenCode, I want the TUI to still work for Claude Code data so that partial failure is not total failure.
14. As a user, I want to see which sources contributed how many records so that I can diagnose missing data.
15. As a future-extending developer, I want a `Source` trait + registry so that adding a new agent (Codex, Aider, Gemini CLI) is one new struct with no changes to dispatch code.

## Implementation Decisions

### Architecture

```
┌─────────────────────────────────────────────────────┐
│  llmhelper binary  (Clap multi-subcommand enum)      │
│                                                     │
│  ┌─────────────┐  ┌─────────────────────────────┐  │
│  │ Source trait │  │  SessionAggregator            │  │
│  │  + registry  │  │  (filter → group → sum)     │  │
│  └──────┬──────┘  └──────────────┬──────────────┘  │
│         │                        │                  │
│  ┌──────▼──────┐         ┌──────▼──────┐           │
│  │ClaudeSource │         │ OpenCodeSrc  │           │
│  │ (JSONL)     │         │ (SQLite)    │           │
│  └─────────────┘         └─────────────┘           │
│         │                        │                  │
│         └────────┬───────────────┘                  │
│                  ▼                                  │
│         ┌──────────────┐                            │
│         │  Record      │ (normalized session)        │
│         └──────────────┘                            │
│                  │                                  │
│         ┌────────▼────────┐   ┌──────────────┐     │
│         │ AppState (Arc)  │──▶│ TuiState    │     │
│         │  + RwLock      │   │  (ratatui)  │     │
│         └────────┬────────┘   └──────────────┘     │
│                  │                                  │
│         ┌────────▼────────┐   ┌──────────────┐     │
│         │ RefreshWorker   │   │ OutputMode   │     │
│         │ (background tokio│   │ (TUI / JSON  │     │
│         │  task + channel)│   │  / CSV)      │     │
│         └─────────────────┘   └──────────────┘     │
└─────────────────────────────────────────────────────┘
```

**Multi-subcommand Clap enum** (`#[command(subcommand)]`) from day one. `usage` is the first entry. Adding `analyze`, `export`, etc. later requires only a new enum variant and a new `run()` dispatch — no main restructuring.

**`Source` trait + registry**: each agent is a `struct` that `impl Source`. The registry (`BTreeMap<String, Box<dyn Source>>`) is built at startup from discovered/override paths. `load()` returns `Vec<Record>`. New agents = new struct + register call, no changes to aggregator or UI.

**Live refresh**: a single `tokio::spawn` background task holds a ticker and re-calls `load()` on each tick, sending new `Vec<Record>` over an `mpsc` channel. The TUI holds `Arc<RwLock<AppState>>` updated on each channel receive; the UI redraws from the lock on each `tick()`. Full re-read each tick — no incremental cache (defer until profiling shows it necessary).

**Degrade per-source**: each `load()` wraps in `Result<Vec<Record>, SourceError>` and returns `Ok(vec![])` with a status field if the source is absent/unreadable. The aggregator tracks `(source_name, record_count, error: Option<String>)` alongside the records. The TUI displays this status panel.

### Normalized `Record` shape

```rust
struct Record {
    session_id:   String,         // source's own id
    source:       String,         // "claude" | "opencode" | "omp"
    project:      String,         // decoded/normalized path
    model:        String,        // raw value — "auto" shown as-is
    agent:        Option<String>, // "build", "research", etc.
    started_at:   DateTime<Utc>,
    ended_at:     Option<DateTime<Utc>>,
    tokens:       TokenBreakdown,
    message_count: u32,
    cost:         Option<f64>,    // opencode + omp (claude has none)
}

struct TokenBreakdown {
    input:      u64,
    output:     u64,
    cache_read: u64,
    cache_write: u64,
}
```

**Token field mapping**:

| Canonical | Claude (per message, summed) | OpenCode (per session) | OMP (per message, summed) |
|---|---|---|---|
| `input` | `input_tokens` | `tokens_input` | `usage.input` |
| `output` | `output_tokens` + `reasoning_tokens` (folded) | `tokens_output` + `tokens_reasoning` (folded) | `usage.output` + `usage.reasoningTokens` (folded) |
| `cache_read` | `cache_read_input_tokens` | `tokens_cache_read` | `usage.cacheRead` |
| `cache_write` | `cache_creation_input_tokens` | `tokens_cache_write` | `usage.cacheWrite` |

**OpenCode `model` normalization**: the `session.model` column is a JSON string (e.g. `{"id":"big-pickle","providerID":"opencode"}`); the adapter extracts the `"id"` field. If the JSON is malformed or missing `"id"`, fall back to the raw string.

**Claude `model`**: present in each assistant-message object (`"model":"auto"`). The adapter takes the **last** non-null `model` value seen in a session's messages as the session-level model. If a session has no assistant messages, `model` is `"unknown"`.

**OMP `model`**: each assistant message carries `model` (often the routing alias `auto`) and a separate `provider` (e.g. `freellm`); the adapter records the **raw `model` value only** (`"auto"`, `"sonnet"`) and takes the **last** non-empty model in the session. `provider` is not folded into the model label, so `--group-by model` groups OMP `auto` with Claude `auto`. OMP records `usage.cost.total` per message; the session `cost` is the sum, but only attached when the total is non-zero — a genuinely free session (0 recorded cost) keeps `cost: None` rather than `Some(0.0)`, avoiding confusion with real spend.

### Data discovery

**Claude Code**: scan `~/.claude/projects/` for directories whose names start with `-`. Each directory name is a hyphen-encoding of an absolute path (e.g. `-home-hunter-projects-modbox` → `/home/hunter/projects/modbox`). Within each directory, find `*.jsonl` files; each is one session. Parse per line, extract `type=="assistant"` messages, sum their `usage` blocks. Skip `type=="mode"` and other non-message lines.

**OpenCode**: read all three DB paths (standard, `-local`, `-dev`) from `~/.local/share/opencode/`. Run `SELECT * FROM session` against each. Union all rows, deduplicating by `id` (same session may appear in two DBs). Convert `time_created`/`time_updated` integers (Unix milliseconds) to `DateTime<Utc>`.

**OMP**: recursively scan `~/.omp/agent/sessions/` for `*.jsonl` files (subagent sessions nest one directory level below the project directory). Project directory names start with `-` — a hyphen-encoding of the path **relative to $HOME** (e.g. `-projects-modbox` → `~/projects/modbox`, bare `-` → `$HOME`). Because every `/` becomes `-`, a hyphen inside a real project name (e.g. `oc-usage`) is **not** recoverable from the directory name alone; the adapter always prefers the session entry's `cwd` for the project path, using the decoded directory name only as a last resort. Each `*.jsonl` file is one session. Parse per line: the `type=="session"` line supplies `id`, `cwd` (project, preferred over the decoded directory name), and `timestamp` (used as `started_at`, preferred over the first message time); `type=="message"` lines with `message.role=="assistant"` supply RFC 3339 timestamps, `model`, and the `usage` block (input/output/reasoningTokens/cacheRead/cacheWrite/cost.total). Sessions with no assistant messages are skipped.

### Config file

`~/.config/llmhelper/config.toml` (XDG-compliant). Created on first write; optional to exist. Format:

```toml
[source.claude]
dir = "/custom/path"  # overrides auto-discovery

[source.opencode]
db = ["/custom/opencode.db"]  # overrides default list

[source.omp]
dir = "/custom/omp/sessions"  # overrides ~/.omp/agent/sessions

[ui]
refresh_interval_seconds = 5
```

Flags always override config values.

### Filter flags (pre-aggregation)

`--since <RFC3339>` — sessions where `started_at >= since`.
`--last <duration>` — sessions where `started_at >= now - duration` (e.g. `7d`, `4h`). Mutually exclusive with `--since`.
`--project <path>` — sessions where `project` contains this path substring.
`--model <pattern>` — sessions where `model` contains this substring (case-insensitive).
`--source <claude|opencode|omp>` — sessions from this source only.
All filters are AND-combined; applied before any aggregation.

### Output modes

- **TUI** (default): ratatui `List` / `Table` layout. Header: grand totals row. Body: table with columns: group key, sessions, messages, input, output (includes reasoning), cache-read, cache-write, cost (per-source only). Token counts render with a magnitude-appropriate unit — raw below 1K, then K, M, B (decimal bases; whole values drop the decimal, e.g. `35.8M`, `318.7K`, `2B`). `Tab` cycles grouping (Source → Project → Model → Source). `r` forces a refresh outside the auto-tick. `q` / `Esc` quits.
- **`--json`**: emit a single JSON object:
  ```json
  {
    "sources": [{"name": "claude", "records": 12, "status": "ok"}, ...],
    "group_by": "source",
    "groups": [{"key": "claude", "sessions": 12, "messages": 84, "tokens": {...}, "cost": null}, ...]
  }
  ```
- **`--csv`**: emit CSV with header row. One row per group. Columns: `group_key, source, sessions, messages, input, output, cache_read, cache_write, cost`.

## Testing Decisions

### Seam: single integration test via `--json` output

The one seam is driving the real `usage` command in `--json` mode against **fixture source directories** (realistic Claude JSONL fixtures + a fabricated SQLite fixture) and asserting the emitted JSON. This exercises: adapter loading → normalization → aggregation → filtering → output serialization, all in one deterministic pass. No TUI involved.

**Fixture structure**:
```
tests/fixtures/
  claude/
    -home-hunter-projects-test/
      session-a.jsonl    # 3 assistant messages, known token sums
      session-b.jsonl    # 1 message, model="auto"
  opencode/
    opencode.db          # 2 sessions, known token sums, cost present
    opencode-local.db    # 1 session (id dup from opencode.db — must deduplicate)
  omp/
    -projects-omp-test/
      2026-08-29T08-00-00-000Z_01fixomp.jsonl  # 1 session, 2 assistant messages, cost present
```

**Assertions**:
- Total session count = 5 (claude 2 + opencode 2 + omp 1). OpenCode deduplicates `opencode.db`/`opencode-local.db` to 2.
- Claude `cost` is `null` in every record; OpenCode and OMP `cost` are `Some` (OMP only when the summed total is non-zero).
- Token sums match fixture values.
- `--source claude` filter emits only Claude groups.
- `--last 7d` filter emits only recent sessions.
- `--group-by model` emits groups with `model` = fixture values.
- Multi-DB merge: the deduplication check.

### Adapter-level unit tests (supplementary)

- Claude JSONL parser: verify `model="auto"` survives as-is; verify each of the five token fields maps correctly.
- OpenCode SQLite adapter: verify `model = '{"id":"x","providerID":"y"}'` normalizes to `"x"`.
- Filter combinator: all-filters-AND and each filter individually.

### What is NOT tested (implementation detail)

The `Source` trait internals, the `tokio` background task lifecycle, the ratatui render loop state machine — none of these are tested directly. Only the external contract: given fixture data and CLI flags, the JSON/CSV output must match expected values.

## Out of Scope

- **Any network egress**: no API calls, no telemetry, no third-party services.
- **Cost aggregation across sources**: cross-source cost sum is explicitly rejected per ADR-0001. A "total cost" cell does not appear in the TUI or JSON.
- **Incremental cache / mtime-based re-read**: full re-read each tick; caching layer deferred.
- **Interactive date picker**: time filtering is flag-only; no in-UI date selection.
- **Tabbed multi-screen**: single-screen totals + table per the design decision.
- **`auto` resolution**: model alias resolution is out of scope; `"auto"` is shown as-is.
- **Any subcommand beyond `usage`** for v0.1.
- **Schema migrations**: OpenCode DB schema is read-only; no migrations.

## Further Notes

- The `session_message` table in OpenCode has a `type` column and a `data` JSON column; we use only the `session` table for token/cost/session metadata. The per-message `session_message` rows are not consumed.
- `Cargo.toml` currently uses `edition = "2024"` (nightly); this should be changed to `edition = "2021"` before `cargo build` will succeed with stable Rust.
- The registry's `Box<dyn Source>` indirection means cloning `Vec<Record>` out of it is the natural pattern; a `Clone` impl on `Record` is needed.
- The live-refresh channel should be bounded (`mpsc::channel(1)`) to drop stale updates if the TUI lags; the TUI always processes only the latest batch.

# LLM Helper

A Rust + Clap command-line tool for Agent/LLM usage introspection. The `usage` subcommand is the first shipped command: it reads local usage data from Claude Code, OpenCode, and OMP, and renders it in a ratatui terminal UI.

## Language

**Usage**:
The token consumption and activity of a single agent session, broken into input, output (including reasoning), cache-read, and cache-write tokens, plus session and message counts and an active time window. Cost is an optional attribute — present only for sources that record it.
_Avoid_: stats, metrics, usage-data

**Source**:
One of the agent tools whose local files the CLI reads. The four known sources are Claude Code (transcript JSONL), OpenCode (SQLite), OMP (per-session JSONL under `~/.omp/agent/sessions`), and Kilo Code (SQLite `kilo.db` under `~/.local/share/kilo`). Each source has a distinct storage format and field coverage. Sources are auto-discovered at default paths and may be overridden by flags or a config file; reading all OpenCode DB variants and merging them is part of Source behavior.
_Avoid_: provider, backend, agent

**Project**:
The working directory a Session ran in. Claude Code encodes it into the transcript folder name; OpenCode stores it in the session's `directory` field; OMP records it in the session entry's `cwd` (with the project directory name as fallback). Used to group Sessions across sources.
_Avoid_: repo, workspace

**Cost**:
The monetary spend attributed to a Session by a Source that records it. OpenCode and OMP expose it directly; Claude Code does not, so its Cost is absent. Cost is always source-scoped — it must never be summed or averaged across Sources, only displayed per Source.
_Avoid_: price, spend, expense

**Token Breakdown**:
The canonical four-part token accounting of a Session: input, output, cache_read, cache_write. Reasoning tokens are folded into `output` at load time, so there is no source-specific token field downstream.
_Avoid_: token-count, token-usage

**Model**:
The model identifier a Session ran against, as recorded by the Source (raw value). Claude Code often records the routing alias `auto`; OpenCode records a resolved id (e.g. `big-pickle`); OMP records the raw `model` value (`auto`, `sonnet`) and keeps `provider` separate. Grouping is by this raw value — `auto` is a known routing-alias bucket, not a resolved model, and is shown as-is rather than guessed.
_Avoid_: engine, model-name

**Predicate Layer**:
One of the four independent exclusion rules a `Filter` applies to the loaded record set: the time window (any of `--since`/`--last`/`--until`/`--calendar`), project, model, and source. The layers are always evaluated in that fixed order, which is what makes the funnel deterministic and the blame layer unambiguous. The window layer is described by its already-resolved absolute bounds, never by the keyword the user typed.
_Avoid_: filter-stage, condition, clause

**Funnel**:
The sequence of record counts observed by applying the Predicate Layers one at a time to the loaded set, each stage built on the previous stage's survivors: `loaded → after window → after project → after model → after source → matched`. The stages are nested by construction, never independent counts against the raw set — cumulative application is what makes "which layer removed the last records" answerable. The **blame layer** is the first stage whose output is zero while its input was non-zero; it is `None` when records matched or when nothing was loaded at all, because "there was no data" is not the filters' fault.
_Avoid_: pipeline, breakdown, trace

**Rank**:
The ordering `compare` puts a window's Groups in, by exactly one explicit metric (`tokens`, `cost`, `sessions`, or `messages`), descending, ties broken by group key so the order is total and the output byte-stable. A Rank is derived from an existing aggregate — it never re-sums — so a group's totals cannot differ between `compare` and `usage`. The fold of the tail past `--top N` is a synthetic `(others)` row, not a Group and never a real key.
_Avoid_: sort, leaderboard, ordering

**Share**:
A ranked Group's chosen metric as a percentage of the metric's grand total in the same window. The Share follows the Rank's sort metric (cost ranks show cost share); the per-field `token_shares` are always by token count. Cost share is measured only against cost-bearing Groups, so a cost-less Group has no Share rather than a misleading `0%` — the ADR 0001 rule applied to a denominator.
_Avoid_: percentage, ratio, proportion

**Budget Annotation**:
The `Over`/`Under`/`NotMeasured` verdict a `Budget` puts on a ranked row in `compare`, matched through the row's group Source. It is an *annotation*, never a predicate: it changes no order, no share, and no exit code, and it is `None` for a `mixed`-source group — a Budget is Source-scoped, so a group spanning Sources has no single spend to be judged against. When no Budget exists for a row's Source the annotation is absent, not `Under`; an unmarked row therefore means "not over", while `NotMeasured` (no Cost recorded) is reported explicitly rather than rendered as `ok`.
_Avoid_: status, flag, alert

**Message Cache**:
The per-file memoization of Message extraction behind `load_messages` (`search` and `export --messages`), keyed by a file's (canonical path, size, mtime-ns, inode). It is a pure accelerator: a cached run and an uncached run produce **byte-identical non-interactive output**; search cache statistics surface under `--explain` (a `cache` object in JSON, one line on stderr otherwise) and in the interactive search header, never in normal `--json`. It is fail-open (any anomaly re-extracts that file), Source-scoped (one NDJSON index per source), and covers only the message corpus — record-level commands never consult it.
_Avoid_: index, store, snapshot, memo






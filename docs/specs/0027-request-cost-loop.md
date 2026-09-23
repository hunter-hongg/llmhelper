---
id: 0027
title: "request — cost-control closed loop: budget gate and the request log as a Source"
status: ready-for-agent
created: 2026-09-22
updated: 2026-09-22
triage: ready-for-agent
---

## Problem Statement

`llmhelper` has two halves that never meet. The **measurement** half — `usage`,
`report`, `compare`, `trend` — reads local agent sources and judges spend
against budgets. The **action** half — `request` — sends Chat Completions
requests to a provider and, if asked, appends them to a per-day log. The two
are connected by nothing: a budget can scream `over` in `report` while
`request` happily keeps spending, and no command can answer "how much did my
own `llmhelper` requests cost?".

Two gaps make this so:

1. **`request` never consults a budget.** Spec 0015 deliberately made budgets
   *annotation only* for the reporting commands ("no nonzero exit on
   over-budget"), which was right for them — a report that refuses to print is
   useless. But it left `request`, the one command that actually spends money,
   with no gate at all. `--budget`/`--budget-window`/`--budget-name` exist on
   `usage`/`report`/`compare` and nowhere that can act.
2. **The request log is write-only.** `write_request_log` appends a JSON line
   per request and per response to `~/.config/llmhelper/logs/request-*.log`
   from five call sites, and *no command reads it back*. It is debug output
   that decays. Yet it is the only place the token counts of `llmhelper`'s own
   requests are recorded — `RequestResponse.usage` is parsed, logged, and
   dropped.

The consequence: cost control in this project is a display, not a control.
The loop is open at both ends — nothing stops an over-budget request, and the
requests themselves are invisible to every measurement command.

## Solution

Close the loop with two additions, each a new consumer of seams that already
exist:

1. **A budget gate on `request`.** `request` gains the three budget flags
   (`--budget`, `--budget-window`, `--budget-name`). Before any payload is
   built or any HTTP call is made, the selected budgets are evaluated against
   loaded Records; if any is `Over`, the request is refused with a new exit
   code **3** and a message naming the budget, the source, the measured spend,
   and the ceiling. The gate is opt-in: with no budget flag given, `request`
   behaves exactly as it does today.
2. **The request log as a Source.** A fifth `Source`, named `llmhelper`, reads
   the per-day log files and emits one `Record` per logged response that
   carries a `usage` block. It is registered like the other four, appears in
   `usage`/`report`/`compare`/`trend`, and can be the target of a budget — so
   `request --budget llmhelper:1.00` gates on the accumulated spend of
   `request`'s own prior runs. That is the loop: measure, gate, spend, log,
   re-measure.

Because the log records tokens and never money, a small **`[price.<model>]`
config table** converts a logged turn's token breakdown into a Cost, which is
the only currency a budget can judge. Without a price entry for a model the
Source reports `cost: None`, and a budget on it evaluates to `NotMeasured` —
absent pricing is not free, exactly as Claude Code's absent cost is `not
measured` rather than zero.

This is an **extension of `request` and of the Source registry**, not a new
subcommand: `request` already owns the log, and the registry already exists so
that "a new source is a single new struct" (ADR 0002).

## User Stories

1. As a developer, I want `llmhelper request --budget opencode:5.00 --prompt
   "..."` to refuse with a clear message when my OpenCode spend for the window
   is already over $5.00, so that a budget can stop a request rather than
   merely decorate a report.
2. As a developer, I want the refusal to exit 3 (distinct from provider
   errors) so that a script or CI job can tell "refused by budget" from
   "provider returned an error" and act differently.
3. As a developer, I want `--budget-name daily` to gate on a budget I have
   configured under `[budget.daily]`, so I do not restate the ceiling and
   window on every invocation.
4. As a developer, I want `usage --source llmhelper` to show the tokens (and,
   with a price table, the cost) of my own `llmhelper` requests, so the tool's
   own spending is measured by the same tool.
5. As a developer, I want `[budget.mine] source = "llmhelper"` to be accepted
   and evaluated like any other source budget, so the loop closes: my next
   request is gated by the accumulated spend of my previous ones.
6. As a developer, I want a price table keyed by model (`input_per_mtoken`,
   `output_per_mtoken`) so that the `llmhelper` source reports cost in the same
   units every other source does.
7. As a developer, I want the `llmhelper` source to be absent from `usage`
   output entirely when I have never enabled request logging, so that a
   non-`request` user sees no phantom empty row.
8. As a developer, I want the log-derived records to be deterministic, so that
   two runs over the same log files produce byte-identical `usage`/`export`
   output and are diffable.
9. As a user, I want a budget that names `llmhelper` while `--log` is off to
   warn me on stderr (not silently pass, not abort) — because unlogged
   requests are invisible to the gate, and a gate that cannot see its own
   spend is a false promise.
10. As a user, I want a corrupted or partially-written log line to be skipped
    with a warning rather than aborting the whole command, so a killed
    mid-write process does not brick measurement.
11. As a user, I want gating to cost nothing when my budget targets one
    source: reading Claude's corpus to check an OpenCode budget would make the
    gate slower than the request it protects.
12. As a developer, I want `request` with no budget flags to be byte-for-byte
    identical to today, so the gate cannot regress the existing contract.

## Implementation Decisions

- **The gate is opt-in and triggers on flag presence.** `RequestArgs` gains
  the three `BudgetArgs`-shaped fields (`--budget <source:amount>` repeatable,
  `--budget-window` defaulting to `1d`, `--budget-name` repeatable). With none
  of the three given, `request` skips the gate entirely and its behavior is
  unchanged. **This differs deliberately from the reporting commands'**
  `resolve_budgets`, where an empty `--budget-name` means "all configured
  budgets": a gate that silently adopted every configured budget would make
  `request` unusable for a user who keeps historical budgets around. Request
  therefore resolves configured budgets only when they are named. The
  per-spec `parse_budget_spec` and `budget::validate_all` are reused; the
  ~fifteen-line resolution wrapper is request-specific rather than a
  flag-riddled shared abstraction.
- **Gate evaluation reuses `budget::evaluate` and is pure.** A new
  `budget::over_budget(budgets, records, now, command_since)` returns only the
  `BudgetState::Over` statuses; it lives in `budget` because it is budget
  logic with no knowledge of HTTP. `command_since` is always `None` from
  `request` — the command has no window filter of its own, so no clipping is
  possible and no `Measurement` is reported. The gate is a single call site in
  `run_request`, placed after validation and before the payload is built, so
  it covers one-shot, `--stream`, and `--interactive` with one check.
- **The gate loads only the sources its budgets name.** When the gate is
  active, `request` builds a registry containing only the sources referenced
  by the resolved budgets (an `opencode` budget does not read Claude's JSONL).
  The `llmhelper` source is registered there under the same existence rule as
  below.
- **A new `RequestError::Budget(String)` maps to exit code 3.**
  `request_exit_code` keeps `Client → 2` and `Request → 1`; the gate refusal
  is neither — it is a pre-flight refusal, before any provider is contacted.
  The message names every over budget: the budget name, the source, the
  measured spend, the ceiling, and the window label, mirroring the prose
  `report` already uses. Like all other errors it goes to stderr as prose and
  exits; JSON mode is not special-cased (see Out of Scope).
- **The `llmhelper` Source emits one Record per logged response.** The log is
  one JSON object per line, `{timestamp, direction, body}` where `body` is a
  stringified JSON payload. The source walks all `request-*.log` files in the
  resolved log directory in sorted order, remembering the most recent
  `direction: "request"` line. For each `direction: "response"` line whose
  parsed body contains a `usage` object it emits a `Record`:
  - `session_id` — `"{file_stem}#{1-based line number}"`, e.g.
    `request-2026-09-22.log#7`. Append-only logs make this stable, so
    re-reading yields identical output.
  - `source` — `"llmhelper"`.
  - `project` — empty string. The log records no project, and inventing one
    would lie. `--project` simply does not match these records.
  - `model` — the response body's `model` field; falling back to the model of
    the remembered request line when the response omits it (some providers do
    not echo it).
  - `started_at` — the envelope `timestamp`; `ended_at` — `None`.
  - `tokens` — `input = usage.prompt_tokens`,
    `output = usage.completion_tokens`,
    `cache_read = usage.prompt_tokens_details.cached_tokens` (0 when absent),
    `cache_write = 0` (the log carries no such field; see Out of Scope).
  - `message_count` — `1`. The Record is one completed turn; the request's
    message array is not paired in, and a per-turn count of 1 is honest.
  - `cost` — `Some(computed)` when the resolved price table has an entry for
    the record's model, else `None`.
  A response line with no `usage` (a non-usage endpoint, a truncated stream)
  yields no Record at all — not a zero-token Record — so unlogged usage stays
  `not measured` rather than diluting a sum.
- **Malformed lines are skipped, not fatal.** A line that fails to parse as
  JSON, or whose `body` is not valid JSON, is skipped; the source counts
  skips and, if any occurred, emits one stderr warning naming the file and
  the count. This is the same "warn, don't abort" posture as a failed Source,
  and it is what makes a log truncated mid-write by a killed process
  recoverable rather than corrupting.
- **`SourceArg` and `KNOWN_SOURCES` gain `llmhelper`.** The enum variant
  `Display`s as `llmhelper`; the budget known-source list gains the same
  string so `[budget.x] source = "llmhelper"` validates. `--source llmhelper`
  on a machine with no logs yields zero records and exit 0, exactly like any
  other source whose directory is absent.
- **Registration follows the existing rule: register when the data exists.**
  The `llmhelper` source is registered when the resolved log directory exists
  and is a directory, mirroring how Claude/OpenCode are registered only when
  their configured paths resolve. A user who never runs `request --log` never
  sees an `llmhelper` row in `usage`; one who has logged sees it
  automatically.
- **The log directory becomes configurable and shareable.** `write_request_log`
  currently derives `~/.config/llmhelper/logs` internally and the source must
  read from the identical location. `[request] log_dir` (optional, default the
  current `log_dir()`) is resolved once, threaded into the request settings,
  and used by both the writer and the source, so they can never disagree.
  Tests drive it with a tempdir through `--config`, as the request tests
  already do for `base_url`.
- **The price table is a keyed config section.** `[price.<model>]` with
  `input_per_mtoken`, `output_per_mtoken`, and optionally
  `cache_read_per_mtoken` / `cache_write_per_mtoken` (each defaulting to the
  input rate when omitted). Cost is `Σ(tokens × rate) / 1_000_000`. Keys are
  matched exactly against the model string the provider echoed — versioned
  suffixes are the user's responsibility to configure (see Out of Scope). A
  model with no entry yields `cost: None`, which makes a budget on it
  `NotMeasured`: this is the same epistemic rule as Claude Code's missing cost,
  applied to missing pricing. The table is loaded as a `BTreeMap`, the second
  keyed table after `[budget.<name>]`, whose precedent established that keyed
  names come from the config key.
- **The `--log`-off warning.** When the resolved gate names a budget whose
  source is `llmhelper` and `--log` is not set, `request` prints one stderr
  line: the budget cannot see unlogged requests. It proceeds (the gate may
  still be satisfiable by other budgets); it does not abort, and it does not
  silently imply the spend figure is complete. Auto-enabling `--log` was
  rejected as too magical — the user's consent to write transcript text to
  disk should stay explicit.
- **`request` remains read-only with respect to usage data.** This is the
  load-bearing constraint from spec 0008's Further Notes. The gate *reads*
  configured sources; it writes nothing to them. The only new write is to the
  request log, which spec 0012 already established. The shift — a read now
  *influences whether the request is sent* — is recorded in ADR 0007, along
  with the rejected alternative of `request` keeping its own spend ledger,
  which would have written usage data and broken 0008 outright.
- **The budget gate evaluates once per invocation, not per turn.** In
  `--interactive` mode the check runs before the first request; follow-up turns
  are not re-gated. Re-reading sources per turn would be heavier than the
  request it protects, and the current turn's spend is not logged until the
  invocation ends. Per-turn gating is Out of Scope.
- **README documents** the gate, exit code 3, the `llmhelper` source, and the
  `[price]` section, and notes that `--log` is what feeds the loop.

## Testing Decisions

- **Unit tests, `src/budget.rs`:** `over_budget` returns only `Over` statuses
  and is a pure function of explicit timestamps — an `Under`/`NotMeasured`
  budget yields an empty vec; a mix yields only the overs, in input order.
  `validate` accepts `source = "llmhelper"` (the `KNOWN_SOURCES`-iterating
  test covers it by construction).
- **Unit tests, the price table:** known tokens + known rates → expected cost;
  a missing model entry → `None`; missing cache rates fall back to the input
  rate; zero tokens → `Some(0.0)` (measured, and `Some(0.0)` is a real spend,
  distinct from `None`).
- **Unit tests, the `llmhelper` source's pure parser:** the seam is a function
  taking log text (one or more files' contents) and returning `Vec<Record>`,
  testable with hand-written lines and no filesystem. Cases: a well-formed
  request/response pair → one Record with correct token mapping; a response
  with no `usage` → no Record; a response whose body is not JSON → skipped and
  counted; a request line with no following response → no Record; `model`
  fallback to the request line when the response omits it; determinism — the
  same input twice yields identical output, including stable `session_id`
  across line numbers; cache tokens absent → 0, not an error.
- **Unit tests, registration:** the source is registered iff the directory
  exists; an absent directory yields `SourceError::Absent`, so `--source
  llmhelper` on an empty machine is zero records, not an error.
- **Integration tests extend `tests/request.rs`, which already runs a local
  HTTP server.** The closing of the loop is asserted end to end in one test:
  1. send `request --log` against the test server into a tempdir `[request]
     log_dir`, where the server's canned response carries a `usage` block;
  2. run `usage --source llmhelper --json` against the same config and assert
     the tokens landed;
  3. run `request --budget llmhelper:0.01` and assert **exit 3** with a
     message naming `llmhelper`, the spend, and the ceiling;
  4. run `request --budget llmhelper:1000.00` and assert it sends and exits 0.
  Additional cases: a configured `[budget.mine] source = "llmhelper"` reached
  via `--budget-name mine` refuses identically; a `[price]` entry makes
  `usage --source llmhelper` report cost, and its absence makes the source
  report nothing measurable; the `--log`-off warning appears on stderr when a
  gate names `llmhelper` and `--log` is unset, and the request still proceeds;
  a budget on a *different* source does not read the log dir; and `request`
  with no budget flags is unchanged (regression guard on the existing request
  tests, which must keep passing untouched).
- **No fixture changes are required** — the loop is exercised by log files the
  tests write themselves into a tempdir; the existing `tests/fixtures` corpus
  is untouched.
- **`compare`/`trend`/`watch` need no new tests** to see the source: they
  consume the registry, and their existing tests cover the general
  "N sources" behavior. The `KNOWN_SOURCES` length is not asserted anywhere,
  so adding an entry does not break a hardcoded count.

## Out of Scope

- **Per-turn gating in `--interactive` mode**, and re-checking a budget after
  each follow-up. The gate is per-invocation.
- **Aborting a live stream mid-flight.** Spec 0009 deliberately parked a
  cancellation token; a stream that turns out expensive still runs to
  completion. This spec does not supersede that decision — a mid-stream abort
  plus fragment preservation is a plausible follow-up, but it is a streaming
  concern, not a budget one, and it would need a new dependency.
- **Machine-readable refusal output.** The gate's refusal is stderr prose plus
  exit 3, like every other `request` error. A `--json` envelope for refusals is
  a follow-up; exit 3 is the scriptable signal for now.
- **A `llmhelper`-side spend ledger.** Rejected: it would write usage data and
  break spec 0008's read-only rule. The loop is fed by the existing opt-in
  `--log` artifact instead. See ADR 0007.
- **Cache-token pricing beyond the input-rate default**, and cache-write
  accounting from the log: the log records no `cache_write` tokens, and
  providers that report them are handled by configuring the rate, not by
  inferring tokens that were never logged.
- **Model-name normalization or aliasing for prices** (mapping a versioned
  echo like `gpt-4o-2024-08-06` to a `gpt-4o` rate). Exact match only.
- **Named request profiles** (`[request.profile.<name>]` switching endpoint,
  key, and model together), and profile-scoped budgets. The current design
  gives the `llmhelper` source a single bucket; a user with two endpoints
  cannot budget them separately. That is a real gap and a natural follow-up,
  but it is configuration ergonomics, not the loop.
- **Retry with exponential backoff** (spec 0008 parked it). It composes with a
  budget gate but is independently large.
- **Anything touching the four existing sources**, the reporting commands'
  budget semantics, or `export`'s output shape. `export` stays header-free and
  budget-free per spec 0015.

## Further Notes

- The loop is **measure → gate → spend → log → re-measure**. The gate sees
  spend accumulated *before* the current request, never the current request
  itself — which is the only correct accounting, since the current request has
  not happened yet. Its cost appears in the *next* invocation's gate.
- Two epistemic rules of the project are reused rather than bent: absent data
  is not zero (an unlogged request is `not measured`, not free; an unpriced
  model is `not measured`, not free), and cost is source-scoped (the
  `llmhelper` source's cost is never summed with OpenCode's — a budget binds
  to one source, per ADR 0001).
- The riskiest edit is `write_request_log`'s signature: it is called from five
  sites in `main.rs`, and the log format is now both a debug artifact and a
  measurement input. The format itself is not changed — existing log files
  parse unchanged — but the function now takes the resolved log directory. As
  with spec 0016's `Field` → `RecordField` rename, that mechanical threading
  should land as its own reviewed step before the source reads from it, so the
  write path is proven unchanged.
- The gate is the first feature to make a `request` outcome depend on local
  usage data. ADR 0007 records why reading is compatible with spec 0008's
  read-only rule and why the ledger alternative was rejected.
- This closes the asymmetry spec 0015 created: budgets annotate `report` and
  gate `request`. The annotation-only decision stands for the reporting
  commands — a report that refuses to print is still useless — and the gate is
  deliberately a separate, opt-in behavior on the command that spends.

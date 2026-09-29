---
id: 0029
title: "source — the llmhelper Source becomes first-class: real Projects and honest Model identity"
status: done
created: 2026-09-27
updated: 2026-09-29
triage: done
---

## Problem Statement

Spec 0027 closed the cost loop: `request --log` writes per-day logs, the
`llmhelper` Source reads them back, `[price.<model>]` turns tokens into Cost,
and a budget gate can refuse a request on the spend it is about to add. The
loop works. The Source is nonetheless a second-class citizen in two ways the
user can observe immediately.

**It has no Project.** Every Record the `llmhelper` Source emits carries
`project: ""` — the log format has no field for it, and `parse_log_file` fills
the empty string deliberately rather than inventing a value (spec 0027). The
consequence is concrete, not cosmetic:

- `usage --group-by project` puts every `request` turn in one anonymous bucket,
  mixed with nothing else — the group key is `""`.
- `usage --project /some/dir` **never** matches a `llmhelper` Record, so the
  tool cannot answer "what did I spend in this project *including my own
  requests*", which is the question the whole loop exists to answer.
- `compare --group-by project --sort-by cost` cannot rank a project by the
  spend that includes direct API calls, because those calls are not attributed
  to any project.

The other four Sources all know their Project: Claude Code encodes it in the
transcript folder name, OpenCode stores a `directory` column, OMP records
`cwd`, Kilo stores `directory`. The `llmhelper` Source is the only one that
does not, and the reason is purely that nobody wrote the field down when the
request was made — information the process had in hand at the time and threw
away.

**Its Model is an unvalidated guess.** `parse_log_file` resolves the model
from the response body, falling back positionally to the most recent
`request` line's `model` field, falling back to `""`. The positional fallback
is a real correctness hazard, not a hypothetical: logs are append-only and
concurrent, and a response that omits `model` is paired with *whatever request
line precedes it* — which, in a log where several requests interleave, may be
a different request. A wrong Model is not a cosmetic error either: it selects
the `[price.<model>]` row, so a mispaired response is priced at the **wrong
rate**, and a budget on the true model silently measures the wrong spend. Spec
0027's "exact match only, no aliasing" decision was about price *lookup*; this
is about the Model *value* itself being unreliable.

## Solution

Make the `llmhelper` Source carry the same two facts the other four do, and
make the one fact that is genuinely absent stay honestly absent.

- **Record the working directory at request time.** The log envelope gains a
  `cwd` field. The `llmhelper` Source reads it and populates `project` with
  the directory's full path, matching how Kilo, OMP, Claude Code, and
  OpenCode present a Project (each records the working directory as a path).
  A log written before this change has no `cwd`, so its Records keep
  `project: ""` — the empty value is then a *fact about the log* (written by
  an older version) rather than a permanent gap, and both behave the same
  way: no Project.
- **Make the model fallback explicit instead of positional.** The `request`
  envelope already carries the model. The `llmhelper` Source stops guessing
  across interleaved lines and only falls back to a remembered request model
  when that pairing is unambiguous; otherwise the Model is empty and, being
  unpriced, the Record is `not measured` — which is the honest answer
  (spec 0027/ADR 0001: absent data is not zero).

The Price mechanism itself does not change: exact-match lookup, `[price.<model>]`
config, `not measured` for an unpriced model. This spec only makes the two
inputs it depends on — *which project* and *which model* — trustworthy.

## User Stories

1. As a user, I want `request` turns to appear under the same Project as my
   Claude Code / OpenCode / OMP / Kilo turns, so a project's total cost
   includes what I spent calling an API directly.
2. As a user, I want `llmhelper usage --project /some/dir` to match my own
   logged requests, so filtering is not silently blind to them.
3. As a user, I want `compare --group-by project --sort-by cost` to rank by
   total spend including direct requests, so "which project ate the budget?"
   has a complete answer.
4. As a user with an old log directory, I want my existing logs to keep
   working (Project empty, not an error), so upgrading does not lose history.
5. As a user whose provider does not echo `model`, I want the tool to use the
   model I actually asked for, so my `[price.<model>]` rate applies.
6. As a user with concurrent/interleaved requests in one log, I want a
   response that cannot be confidently paired to report an unknown Model (and
   therefore `not measured`) rather than charge me at the wrong rate.
7. As the author, I want a Record's Project and Model to be recorded facts
   from the request, not values reconstructed by position in a text file.

## Behaviour

### `cwd` in the log envelope

- `write_request_log` gains a `cwd` field on the envelope, set to the process's
  current working directory. The envelope's existing keys (`timestamp`,
  `direction`, `body`) are unchanged; `cwd` is **added**, so old readers that
  ignore unknown keys keep working and the log format is append-compatible.
- The `llmhelper` Source reads `cwd` and sets `project` to the directory path
  **as-is**, matching Kilo, OMP, Claude Code, and OpenCode — which all store a
  full working directory (`/home/u/code/api`), not a name. An empty or absent
  `cwd` (an old log) leaves `project` empty. Using the full path is what makes
  `--project /some/dir` match a logged request and `--group-by project`
  aggregate turns across sources; a basename would split the same directory
  into two buckets depending on which Source produced the turn.
- The API key is still never written; `cwd` is a path the user already knows.

### Honest model pairing (consume-once)

- A `response` line's Model is: the response body's `model` if present;
  otherwise the model held by the most recent unconsumed `request` line.
  That `request` line is then consumed and cannot supply a model to any
  later `response`. If a `response` has no echo and no unconsumed `request`
  precedes it, the Model is empty — never a stale value from an
  earlier, already-consumed request.
- **Pairing rule (same-file, positional, consume-once):** a `request` line
  supplies its model to the next `response` line that lacks one, and is then
  taken. This is byte-identical to the old positional fallback for the common
  case (one request → one response). It is strictly more honest under
  **interleaving**: `request(A) request(B) response response` lets the first
  response take B and leaves the second response empty (`["B", ""]`), which is
  mispriced on B but never silently priced on A's rate — the second error is
  the old one being fixed, the first is accepted as the format's limit.
- An empty Model means `cost_for` finds no price → `cost: None` → a budget on
  `llmhelper` is `not measured`, exactly as spec 0027 specifies for an
  unpriced model.


### What does not change

- The `[price.<model>]` table, exact-match lookup, and the cache-rate
  fallbacks are untouched (spec 0027 / `src/price.rs`).
- Token extraction, `cache_write: 0`, `ended_at: None`, and the
  `"{file}#{line}"` session id are untouched.
- The Source still registers only when its log directory exists.

## Out of Scope

- **Model-name normalization / aliasing** (`gpt-4o-2024-08-06` → `gpt-4o`).
  Spec 0027 ruled exact-match out of scope and this spec does not reopen it;
  a user with a versioned echo still writes that exact string as their
  `[price.…]` key.
- **Project normalization.** The four other Sources store a directory *path*,
  not a name — Claude Code's decoded folder name is a full path, OMP's `cwd`
  field is a full path, and Kilo/OpenCode's `directory` column is a full path.
  This spec follows them and stores the `cwd` as-is. Normalizing either
  direction is not defined here: trimming a trailing separator is; collapsing
  `..` or symlinks is out of scope, as is reducing a path to a basename (which
  would be inconsistent with every other Source).
- **Request profiles / per-endpoint budgets.** Spec 0027 filed this as a real
  gap and a natural follow-up; it is configuration ergonomics, not Source
  identity, and it stays filed there.
- **A `llmhelper` spend ledger.** Rejected in ADR 0007; the loop is fed by the
  opt-in `--log` artifact, not a second source of truth.
- **Migrating old logs to add `cwd`.** Historical requests were made in
  directories the log never recorded; reconstructing them would be invention.
  Old Records keep an empty Project and that is the correct, honest state.

## Architectural Decisions

- **`cwd` is a new envelope key, not a new log file or format version.**
  Adding a key to an append-only JSON-lines format is backward- and
  forward-compatible: a reader that ignores unknown keys (all of them) is
  unaffected, and old lines simply lack the field. A version field and a
  migration path would be a heavier contract for a strictly additive change.
- **The model fallback becomes consume-once, not most-recent-wins.** The
  current "most recent request line" rule is correct only when the log is
  strictly one-request-then-one-response. Making the fallback consume the
  request it uses is the smallest change that makes an interleaved log
  *conservative* (unknown Model → unmeasured) rather than *wrong* (a price
  from the wrong request). The tool's stated preference — an unmarked value
  is "not over", `not measured` is reported explicitly, absent is not zero —
  only holds if the pairing is honest.
- **Project is a directory path, consistent with every other Source.** Claude
  Code, OMP, Kilo, and OpenCode all store a full working directory, so the
  llmhelper Source stores one too. The directory name is not a Project value
  anywhere; using only the tail here — while the other Sources store the path
  — is what would split the same directory into two buckets depending on which
  Source produced the turn. The aggregator's `mixed` handling (ADR 0001) is
  about Source cost attribution, not project formatting; cross-Source
  consistency here comes from matching the path, not from invoking `mixed`.
- **The empty-Project old-log case is not backfilled.** The alternative —
  attributing old requests to the project the user is currently in — would
  put spend in the wrong place. Absent stays absent.

## Testing Decisions

- Unit tests in `src/source.rs` for the `llmhelper` Source: a log with `cwd`
  populates `project` to the directory **path** as-is; a log without `cwd`
  leaves it empty; an envelope with `cwd` but no path (e.g. `/`, empty, or a
  Windows root) yields an empty Project rather than a root.
- Model pairing: response with `model` wins; response without `model` uses the
  immediately preceding request's model; a second response with no `model` and
  no unconsumed request does **not** reuse the earlier request's model (the
  regression this spec exists to fix). An interleaved log is *conservative*,
  not correct: the first response still pairs to the later request's model (the
  test `an_interleaved_log_never_invents_a_second_model` asserts this openly —
  `["mistral", ""]`), so it is mispriced; the second is unpriced. Recovering
  the first pairs needs a request id the log does not carry (ADR 0009).
- Unpriced/empty Model yields `cost: None` (reuse the existing
  `unpriced_model_is_not_measured_so_the_gate_cannot_refuse` shape).
- An integration test that runs `request --log` through the real binary and
  asserts the `llmhelper` Source reports a non-empty Project for the round
  trip, so the envelope write and the reader are tested together (highest
  seam that catches the regression).
- The existing byte-identity/re-read stability of the append-only log is
  re-asserted after the format gains a key.

## Further Notes

- This spec makes the `llmhelper` Source a peer of the other four on the two
  fields the tool groups and filters by (Project, Model). It does not attempt
  to make it a peer in every respect — it still records `ended_at: None` and
  `cache_write: 0` because the provider protocol does not report them, which
  is spec 0027's "handled by configuration, not inferred" stance.
- The `cwd` key is written unconditionally when `--log` is on; it is the
  cheapest possible field and the one whose absence is the actual gap.

---
id: 0020
title: "Why did I get no results? (filter diagnostics)"
status: done
created: 2026-09-03
updated: 2026-09-25
triage: done
---
# 0020 — Why did I get no results? (filter diagnostics)

## Problem

Every read-only command scopes its records through the same four-layer
predicate stack (`--since`/`--last`/`--calendar` window, `--project`,
`--model`, `--source`). When a combination matches nothing, the user gets a
silent empty result and no way to tell *which* layer removed the data:

```
$ llmhelper usage --source omp --project /typo-here --csv
group_key,source,sessions,messages,input,output,cache_read,cache_write,cost
$                                            # header only, zero explanation
```

The failure is worse than "unhelpful", it is **actively misleading**:

- `usage --json` still emits a populated `sources[]` array. Those numbers are
  the *loaded* counts, not the matched counts, so the output appears to contain
  data even though nothing matched. A downstream tool reading `sources[].records`
  sees `22` and concludes there was usage. (Addressed by spec 0021, which adds
  a `matched` count per source alongside the loaded one.)
- The three output modes disagree. The table path prints `(no groups)`
  (`output.rs`); CSV prints a bare header; JSON prints an empty `groups: []`
  beside the loaded `sources[]`. The same empty result looks like three
  different situations.
- Nothing distinguishes **"the filters excluded everything"** from **"there was
  nothing on disk to begin with"** — an empty `--claude-dir` and an
  over-narrow `--project` render identically.

The user's only recourse is to delete flags one at a time and re-run, guessing.
The pipeline already computes everything needed to answer this; it just throws
it away.

## Stories

1. **A user gets an empty result and learns which layer caused it.** After
   `usage --source omp --project /typo --last 1d`, the output states how many
   records were loaded, how many survived each progressively-applied predicate,
   and names the first predicate that removed everything.

2. **"No data at all" is distinguishable from "filtered to nothing."** When
   zero records were loaded, the diagnostic says so and does **not** blame a
   filter. When records were loaded but all were excluded, it blames the
   filters.

3. **The explanation appears in all three output modes.** Table, `--csv` and
   `--json` agree on whether the result is empty and on the cause. JSON carries
   it structured, inside the payload; CSV has no structured channel that would
   not corrupt the table, so it explains on stderr and leaves the body a bare
   header.

4. **`--explain` shows the full per-layer funnel even on a non-empty result.**
   A user who wants to know "how much did `--project` cost me?" can ask for the
   breakdown without first emptying the result.

5. **A single dominant filter is named when it alone removes everything.**
   Given several active predicates, if one is solely responsible, the message
   names that one rather than listing all of them.

6. **Filtering by a value that no record has is reported distinctly from a
   filter that merely narrows.** `--source omp` when no OMP source was
   discovered is a different situation from `--source omp` when OMP loaded 300
   records but the time window excluded them.

7. **Nothing changes for a non-empty result without `--explain`.** Existing
   output stays byte-for-byte identical; the diagnostic is additive.

## Domain concepts

**Predicate layer**: one of the four independent exclusion rules a `Filter`
applies — `window` (any of `since`/`last`/`until`), `project`, `model`,
`source`. They are evaluated in a fixed order (`window`, then `project`, then
`model`, then `source`) so the funnel is deterministic and testable.

**Funnel**: the sequence of record counts observed by applying the predicate
layers one at a time to the loaded record set, each stage built on the previous
stage's survivors:

```
loaded → after window → after project → after model → after source → matched
```

The funnel's stages are **nested by construction**, never independent: applying
layers cumulatively is what makes "which layer removed the last records"
answerable. A per-layer count computed in isolation (each predicate alone
against the loaded set) would not compose, and would misreport when several
predicates overlap.

**Blame layer**: the first stage whose output count is zero while its input
count was non-zero — the layer that turned a non-empty set into an empty one.
`None` when records matched, or when there were no records to begin with.

## Behaviour

- **When the result is empty**, every read command emits a diagnostic that
  names the loaded count, the matched count (`0`), and the blame layer with its
  predicate value, e.g.:
  `no records matched — loaded 421, excluded by --project "/typo-here"`, with
  the full funnel on the following line.
- **When zero records were loaded**, the diagnostic says
  `no records loaded from any source` and blames no filter, even if predicates
  are active.
- **When records matched**, no diagnostic appears unless `--explain` is given.
- **`--explain`** always prints the full funnel, for empty and non-empty results
  alike. It is a boolean flag available on each read command.
- **The diagnostic goes to stderr in table mode** so stdout stays clean for
  piping, and is embedded in the payload for `--json` and `--csv` (see below).
  Rationale: a table-mode user is reading the terminal, and a JSON consumer must
  not be forced to parse prose from stderr to learn that their query was empty.
- **Exit code stays `0`.** An empty result is not an error — narrowing to
  nothing is a legitimate outcome, and `search`/`usage`/`report` already treat
  it as success. Turning it into exit 1 would break every existing script.

### Structured forms

- **`--json`**: add a `diagnostics` object, present only when the result is
  empty *or* `--explain` was passed:

  ```json
  "diagnostics": {
    "loaded": 421,
    "stages": [
      {"layer": "window", "value": "2026-09-11T12:00:00Z .. 2026-09-12T12:00:00Z", "remaining": 88},
      {"layer": "project", "value": "/typo-here", "remaining": 0},
      {"layer": "model", "value": null, "remaining": 0},
      {"layer": "source", "value": null, "remaining": 0}
    ],
    "matched": 0,
    "blamed": "project"
  }
  ```

  `value` is `null` for an inactive layer, and inactive layers carry the
  previous stage's `remaining` forward. `blamed` is `null` when nothing is
  blamed.

- **`--csv`**: the existing rows are unchanged. When the result is empty, the
  existing diagnostic is written to **stderr** and the CSV body remains a bare
  header, so the file parses as a well-formed (if empty) CSV. A comment line
  inside a CSV would corrupt it for strict parsers; JSON is the
  machine-readable channel.

  Rationale for the asymmetry: JSON is a structured document that can carry an
  extra field without breaking consumers, while CSV is a flat table whose only
  safe empty form is a header with no rows.

- **`sessions --detail <id>`**: a missing id already errors explicitly
  (`session id not found`); it gains the funnel on stderr for consistency, but
  the error and exit code are unchanged.

- **`search`**: already prints `No matches for "query"`. It keeps that wording
  (it is query-shaped, not layer-shaped) and gains the funnel **only** under
  `--explain` or when records were loaded but filters excluded them all.

- **`watch`**: the TUI re-renders every interval, so a per-frame diagnostic
  would flicker and spam. `watch` shows the funnel **only** in its `--json`
  frame (which is byte-identical to `usage --json`, so the field must be added
  to both) and never in the TUI. The TUI's job is the live picture; an empty
  live picture is already visible.

## Architectural decisions

- **The funnel is computed from the same `Filter` that did the filtering.** No
  second, parallel predicate implementation. This is the whole reason the
  feature is trustworthy: if the diagnostic's idea of "excluded" diverged from
  the loader's, it would confidently report a wrong cause. The counts come from
  applying the *actual* `Filter` stages, and a test asserts the funnel's final
  stage equals the aggregate's matched session count.

- **New `src/diagnostics.rs`, pure.** `diagnose(records, filter) -> Diagnostics`
  takes the loaded records and the filter and returns the funnel. It reads no
  clock, touches no I/O, and returns plain data; rendering is a separate pure
  function. This is the established shape (`budget::evaluate`,
  `search::search`, `domain::window::window_bounds`).

- **The window layer is described by its resolved bounds, not re-resolved.** By
  the time records are loaded, the `Filter` already carries concrete
  `since`/`until`. The diagnostic never calls `window_bounds` or `Utc::now()`;
  it reports the resolved interval. A re-resolution could disagree with the
  filter that actually ran (the clock moved), which is precisely the bug class
  spec 0018 fought.

- **`Filter.last` is deliberately not used by the diagnostic.** `Filter::last`
  is evaluated against an internal `Utc::now()` in `matches_at`, so a funnel
  stage built on it would not be reproducible. Commands resolve `--last` into
  `since`/`until` via `window_filter` before filtering, so the diagnostic only
  ever sees absolute bounds. A diagnostic that needed `last` would be a design
  error, and the code will not have a code path for it.

- **Counting is by record, not by session or token.** Records are the unit the
  predicate stack operates on, so a record-count funnel describes the filters
  exactly. Token-weighted funnels would confuse "one huge session excluded" with
  "all sessions excluded".

- **The diagnostic is computed once, next to the aggregate.** `AggregateResult`
  is built from `(records, filter)`, and the diagnostic needs the same two
  inputs; computing both at that point keeps them consistent by construction and
  avoids a second pass over the data.

- **No exit-code or output-shape change on the success path without
  `--explain`.** Byte-for-byte compatibility for non-empty results is a hard
  requirement, verified by regression tests, because `watch --json` is asserted
  equal to `usage --json` and downstream tools parse both.

## Output formats

The funnel is one line in table mode:

```
no records matched — loaded 421, excluded by --project "/typo-here"
  filters: loaded 421 · window (2026-09-11T12:00:00Z .. 2026-09-12T12:00:00Z): 88 left · project ("/typo-here"): 0 left · model: — · source: —
```

The window stage reports its **resolved absolute bounds**, never the keyword the
user typed: records resolve `--last` before filtering, and re-deriving a keyword
here could disagree with the filter that actually ran.

With `--explain` on a non-empty result, the same `filters:` line appears without
the "no records matched" sentence.

## Error Handling

- The diagnostic never fails a command. It is derived from data already in
  memory; there is no new error path and no new `anyhow::bail!`.
- `--explain` on a command that has no filter flags is a no-op flag, not an
  error: it keeps the CLI uniform and costs nothing.

## Testing Decisions

- **Funnel unit tests** (`src/diagnostics.rs`): empty load blames nothing;
  a single exhaustive layer is blamed; nested stages count correctly when two
  predicates overlap (proving nesting, not independent counting); the final
  stage equals `filter.apply(records).len()` (the anti-drift assertion);
  `value` is `None` for inactive layers; no clock is read (the function has no
  `now` parameter, which makes this structural rather than asserted).
- **Render unit tests**: the one-line form for each blame layer, the
  zero-loaded form, the `--explain` funnel line, and that `model`/`source`
  inactive stages render `—`.
- **Integration tests**: for each read command, assert the diagnostic appears on
  stderr when an over-narrow filter is applied and points at the right layer;
  assert it is *absent* on a non-empty result without `--explain`; assert
  `--json --explain` carries `diagnostics`; assert a non-empty `--json` result
  is unchanged; assert exit code is `0` in every empty case; assert the
  zero-loaded case blames no filter.
- **The `watch --json` equality test is extended**, not duplicated: since
  `watch --json` must equal `usage --json` byte-for-byte, the `diagnostics`
  field must be identical in both, and the existing equality assertion covers it.
- **Real-data smoke**: run `usage --project /nonexistent --last 1h --explain`
  against the machine's own sources (12k+ messages across four sources) and
  confirm the funnel totals match the loaded counts.

## Out of Scope

- **No auto-correction or suggestion.** The diagnostic reports what happened; it
  does not propose `--project /probably-this`. Guessing is a different feature
  with its own failure mode (a confident wrong suggestion is worse than silence).
- **No exit-code change.** See Behaviour.
- **No token- or cost-weighted funnel.** See Architectural decisions.
- **No persistent query log or history.** The diagnostic is derived per run.
- **No diagnostics for `request`.** It filters nothing: it sends a prompt to an
  endpoint. Its empty-input error (`provide a prompt via --prompt or
  --messages`) is already explicit and unrelated.
- **No diagnostics inside any TUI view.** The TUI is not a shell; a reason line
  belongs where a shell can read it. `report`'s TUI renders the funnel as part
  of the Markdown document when `--explain` is passed, since that document *is*
  the report.
- **No change to `export`.** It is a projection over already-filtered records
  with no aggregate; it inherits whatever the loader produced and its empty
  output is a valid empty export.

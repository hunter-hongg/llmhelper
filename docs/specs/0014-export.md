---
id: 0014
title: "export — flat record dump for downstream tools"
status: done
created: 2026-09-12
triage: done
---

## Problem Statement

Every existing subcommand answers a question for a human. `usage` and `diff`
aggregate, `sessions` lists readable rows, `report` renders a shareable document,
and `search` matches message text. None of them emits the normalized `Record`
set itself in a shape a downstream program can consume directly.

`sessions --json` is the closest thing, but it is (a) entangled with the
interactive command's paging/detail flags, (b) rendered as one pretty-printed
array with no per-line framing, and (c) fixed to a column layout the reader
cannot choose. A script author who wants "give me every session in the last
30 days as one JSON object per line, with only the fields I asked for" has to
reach for `jq` re-shaping of an aggregate-oriented output.

There is a missing symmetry: `report` is the command for *humans* who want a
shareable view, and there is no command for *programs* that want the raw
normalized data.

## Solution

Add a sixth subcommand, `llmhelper export`, that loads all Sources through the
existing registry, applies the same `Filter` vocabulary as the other read-only
commands, and emits the resulting `Record` set to stdout in a machine-oriented
format — one Record per row, no aggregation, no TUI, no subtitle prose.

```
llmhelper export --last 30d --format jsonl
llmhelper export --format csv --fields source,project,model,input,output,cost
```

The default format is `jsonl` (one JSON object per line), because export's
consumer is always a pipe or a file, never a human at a keyboard. `--format`
selects `jsonl`, `json`, `csv`, or `tsv`. `--fields` selects which columns a
row carries; with no `--fields`, every field is emitted in the canonical order.

The command is read-only and side-effect-free: it writes to stdout and exits.
It never aggregates, so Cost stays attached to the individual Record that
recorded it, and the source-scoped Cost invariant is upheld by construction —
there is no place to sum across Sources.

## User Stories

1. As a script author, I want `llmhelper export --format jsonl` so that I can stream every Session as one JSON object per line into another tool.
2. As a script author, I want one Record per line by default so that I can pipe export through `grep`, `head`, or a JSON-lines reader without re-shaping.
3. As a script author, I want `--format json` so that I can consume the whole set as a single parseable array when I do not need streaming.
4. As a script author, I want `--format csv` so that I can open the export in a spreadsheet or load it with a CSV reader.
5. As a script author, I want `--format tsv` so that I can feed tab-separated output to tools that mishandle quoted CSV.
6. As a script author, I want `--fields` so that my rows carry only the columns I asked for, in the order I asked for them.
7. As a script author, I want an unknown `--fields` name to be a clear error naming the bad field so that a typo fails loudly instead of silently dropping a column.
8. As a script author, I want the canonical field order when `--fields` is omitted so that the default shape is stable and documented.
9. As a script author, I want every filter the other commands accept (`--since`/`--last`, `--project`, `--model`, `--source`) so that I scope an export with the same vocabulary I already use.
10. As a script author, I want `--last` and `--since` to remain mutually exclusive so that the exported window is unambiguous.
11. As a script author, I want Source-load errors reported on stderr so that a partial export is visible and never silently mistaken for the complete set.
12. As a script author, I want a Source that failed to load to be distinguishable from a Source that held no records, so that I do not read a load failure as an empty result.
13. As a script author, I want a deterministic row order so that two exports of the same data diff cleanly.
14. As a multi-machine user, I want the same source-path override flags and config file support as the other commands so that export works wherever my data lives.
15. As an agent user, I want export to be silent on stdout except for the records themselves, so that I can redirect it to a file without stripping a banner.
16. As an agent user, I want the raw Model value (including the `auto` bucket) exported unchanged so that export never invents a resolved model.
17. As an agent user, I want Cost omitted or null for Sources that record no spend so that missing spend data is never exported as `0`.
18. As an agent user, I want timestamps in RFC 3339 UTC so that downstream parsers read them without guessing a format.
19. As an agent user, I want an empty result set to be a valid, success exit with no rows so that a scoped export that matches nothing is not an error.
20. As a developer, I want the row emitter to be a pure function from `(records, format, fields)` to bytes so that it is unit-testable without a terminal or a real source tree.
21. As a developer, I want export to reuse `Registry`, `Filter`, `Record`, and the Source-status convention so that no parallel data model is introduced.
22. As a developer, I want export to be a sibling of `report` — one for machines, one for humans — over the same pipeline, so that the two never disagree about what a filtered record set is.

## Implementation Decisions

- **New `ExportArgs` in `cli.rs`** mirroring the standard read-only filter
  flags (`--claude-dir`, `--opencode-db`, `--omp-dir`, `--kilo-db`, `--since`,
  `--last`, `--project`, `--model`, `--source`) plus:
  - `--format <jsonl|json|csv|tsv>`, a `ValueEnum` defaulting to `jsonl`.
  - `--fields <list>`, optional, comma-separated or repeatable, naming the
    columns to emit.
- `--since` and `--last` are mutually exclusive, validated with the same
  message as the other commands.
- **A new `src/export.rs` module owns emission.** The public seam is a pure
  function `render<W: Write>(records: &[Record], opts: &ExportOptions, out: &mut W) -> anyhow::Result<()>`
  where `ExportOptions` carries the resolved `format` and the resolved field
  list. It performs no I/O beyond writing to the passed writer and no Source
  access, mirroring `report::render_report` and `output::render_diff_csv`.
- **A canonical field set** is defined once as an ordered list. Every field is
  a column derivable from a `Record`:
  `source`, `session_id`, `project`, `model`, `agent`, `started_at`,
  `ended_at`, `messages`, `input`, `output`, `cache_read`, `cache_write`,
  `cost`. `--fields` selects a subset and fixes the column order; an unknown
  name is an error that names the offending field and lists the valid names.
- **Format semantics:**
  - `jsonl` — one compact JSON object per Record, one per line. Key order is
    fixed by the resolved field list; a selected field that is absent is
    emitted as `null` (e.g. `agent`, `ended_at`, `cost` on a source that does
    not record them).
  - `json` — a single pretty-printed JSON array of the same objects.
  - `csv` / `tsv` — a header row of the resolved field names, then one row per
    Record. CSV quoting is handled by the `csv` crate; TSV uses the same crate
    with a tab delimiter. Cell values are formatted for human diffing
    (`format_tokens` is **not** used — cells are raw integers so that a
    spreadsheet sees numbers, not `12.3M`). Cost is the raw float with six
    decimals, empty when absent.
- **Row order is deterministic:** by `started_at` descending, tie-broken by
  `(source, session_id)` ascending. This makes repeated exports of identical
  data byte-identical, which is the point of a machine format.
- **Cost is per-Record and never summed.** A Record whose Source records no
  spend carries no `cost` (JSON `null` / empty CSV cell), so the source-scoped
  invariant is structural rather than a rule to remember.
- **`main.rs` gains `run_export(args, config_path)`**: load config merged with
  CLI overrides through the existing path, discover sources, build the filter
  (same construction as `usage`/`report`), load once, apply the filter, sort,
  and call `render` on `std::io::stdout()`. Source-load errors print to
  stderr as `warn: source <name> error: <err>`, matching the degrade-per-Source
  convention; they are never fatal. Exit code is 0 even when zero rows match.
- **Export is non-interactive.** There is no TUI and no `--json`/`--csv` alias
  flags; `--format` is the single output selector, because export's consumer
  is a program and an interactive view already exists (`usage`).
- **No new dependencies.** The `csv`, `serde_json`, and `clap` crates already
  present cover every format.

## Testing Decisions

- **Unit tests** in `src/export.rs` target the pure `render` function with
  hand-built `Record` values — the seam from Implementation Decisions:
  - JSONL emits exactly one line per Record and each line parses as a JSON
    object.
  - Canonical field order is stable when `--fields` is omitted.
  - `--fields` reorders and subsets columns; each requested key is present.
  - An absent optional field (`agent`, `ended_at`, `cost`) serializes as JSON
    `null` and as an empty CSV/TSV cell, never `0`.
  - CSV/TSV emit a header row matching the resolved fields and one row per
    Record.
  - Token cells are raw integers, not `format_tokens` magnitude strings.
  - Row ordering is `started_at` descending, tie-broken by `(source, session_id)`.
  - An empty record slice emits a valid empty result: no JSONL lines, `[]` for
    JSON, header-only for CSV/TSV.
  - An unknown `--fields` name errors and the message names the bad field.
- **Integration tests** live in `tests/export.rs`, run the built binary
  against the existing `tests/fixtures` tree with the four `--*-dir`/`--*-db`
  overrides, and assert: exit success, JSONL line count equals the record
  count, each line is parseable and carries the expected keys, `--format json`
  is a parseable array, `--format csv`/`tsv` have a header and the right
  delimiter, `--fields` narrows and reorders columns, each filter
  (`--source`, `--project`, `--model`, `--since`, `--last`) narrows the set,
  `--since`/`--last` together error, an unknown `--fields` errors with exit 1,
  and a zero-match filter exits 0 with empty output.
- **Prior art:** the integration harness mirrors `tests/integration.rs` and
  `tests/request.rs` (spawn `CARGO_BIN_EXE_llmhelper` with fixture paths);
  the pure-renderer unit style mirrors `src/report.rs`.
- Existing tests must pass unchanged; no behavior of `usage`/`diff`/`sessions`/
  `report`/`search`/`request` may change.

## Out of Scope

- Any output format other than `jsonl`/`json`/`csv`/`tsv` (no Parquet, no
  SQLite, no Excel).
- Aggregation, grouping, or totals inside export; that is `usage`/`report`.
- Writing to a file path (`--output`); stdout redirection is the shell's job.
- A TUI or interactive mode.
- Exporting message text (`search`'s corpus) or reasoning content; this spec is
  Record-level only. Message export is now specified separately in
  `docs/specs/0016-message-export.md` (`export --messages`), which reuses this
  spec's format and `--fields` plumbing.
- Cross-Source Cost totals or model-alias resolution (permanent project
  conventions).
- Scheduling, streaming beyond stdout, or network destinations.

## Further Notes

- `export` completes the `report`/`export` symmetry: both consume the same
  filtered `Record` set, `report` renders it for a human and `export` renders
  it for a program. The pipeline change is a new `Command` variant plus a pure
  emitter — no changes to `Source`, `Registry`, `Filter`, or `Record`, which is
  the architecture working as intended.
- Unlike `report`, export deliberately does **not** print a header, a
  timestamp, or a filters-echo on stdout: any such line would break a JSONL
  consumer. The applied filters are recoverable from the invocation, and
  Source errors go to stderr.

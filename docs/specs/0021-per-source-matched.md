---
id: 0021
title: "Per-source matched counts"
status: done
created: 2026-09-09
updated: 2026-09-25
triage: done
---
# 0021 — Per-source matched counts

## Problem

Spec 0020 diagnosed why an empty result looked misleading, but deliberately
did not touch the field at the centre of the confusion. `usage --json` reports
`sources[].records`, and those numbers are **loaded** counts, not **matched**
counts. On an empty result the output therefore reads as:

```json
{
  "sources": [
    {"name": "claude",  "records": 22,  "status": "ok"},
    {"name": "codex",   "records": 149, "status": "ok"},
    {"name": "cursor",  "records": 81,  "status": "ok"},
    {"name": "opencode","records": 40,  "status": "ok"}
  ],
  "groups": [],
  "diagnostics": { "loaded": 292, "stages": [...], "matched": 0, "blamed": "project" }
}
```

`groups` is empty, `diagnostics.matched` is `0`, and yet the `sources` panel
carries 292 records. A downstream tool that reads `sources[].records` to decide
whether there was usage concludes there was. Spec 0020 fixed the *human*
readability of this (the stderr message and the funnel) and left the field as
it was, explicitly noting that it "appears to contain data even though nothing
matched".

Two facts make this unfixable by editing prose:

- `sources[].records` is emitted at **two** sites (`usage` and `diff`) through
  one shared `SourceInfo` struct, so the ambiguity is structural, not a bug in
  one call site.
- A test pins the semantic: `funnel_loaded_equals_the_sum_of_source_records`
  asserts `diagnostics.loaded == sum(sources[].records)`. The field is *the*
  loaded count by contract, so it cannot be redefined to mean "matched".

## Stories

1. **A JSON consumer can tell "this source contributed" from "this source was
   loaded".** For the frame above, each source gains a `matched` field that
   counts how many of its own records survived the filter, so the empty result
   shows `records: 22, matched: 0` for every source instead of a bare `22`
   beside an empty `groups`.

2. **Loaded and matched sit on the same row.** The ambiguity in 0020 was a
   reader having to notice that two arrays in the same payload disagree. Putting
   both counts on the `sources` entry means the discrepancy is visible without
   cross-referencing `diagnostics`.

3. **Per-source `matched` reconciles with the funnel.**
   `sum(sources[].matched) == diagnostics.matched`, so a consumer can verify the
   two views agree rather than trusting either one alone.

4. **An ordinary non-empty run is byte-identical to before.** The additive field
   is emitted only in the same runs that already carry `diagnostics` — an empty
   result or `--explain`. Every other frame gains no key, so the byte-identity
   guardrails that protect downstream parsers still hold.

5. **`watch --json` remains byte-identical to `usage --json`.** Both render
   through the one shared renderer, so the new field lands in both at once and
   the invariant is preserved by construction rather than by a duplicated
   change.

## Behaviour

- **`matched` is present only when `diagnostics` is.** The field is
  `Option<usize>` with `skip_serializing_if = "is_none"`, and the renderer
  consults the per-source map only when the funnel is carried
  (`diagnostics.and(matched_by_source)`). This is not an accident of
  serialisation: it is the exact set of runs where the loaded/matched
  distinction matters, and it is what keeps ordinary frames unchanged.

- **A source that matched nothing reports `matched: 0`** rather than omitting
  the key. On an empty result every discovered source should show `0` on the
  same row as its loaded count; omitting would reintroduce the "is it absent or
  zero?" reading the funnel exists to remove.

- **`records` keeps its meaning.** It remains the loaded count, unchanged, so
  `diagnostics.loaded == sum(sources[].records)` stays true and 0020's
  reconciliation holds.

- **`diff --json` omits `matched`.** `render_diff_json` passes no map. The
  funnel is a `usage`/`watch` concept, and the diff frame's sources panel is
  just provenance for which loaders ran.

## Architectural decisions

- **Additive, not a rename.** Renaming `records` to `records_loaded` would be
  the most honest fix, but it breaks every existing JSON consumer and collides
  with the byte-identity guardrails the repo maintains on this output. Adding
  `matched` alongside leaves the loaded count's readers untouched while giving
  them an honest counterpart.

- **`matched_by_source` reuses `Filter::apply`, never a parallel predicate.**
  This is the same discipline as 0020: a second reimplementation of the four
  layer predicates could drift from the aggregate's and report a confident wrong
  number. Counting the output of `filter.apply` cannot drift.

- **It is a pure function in `src/diagnostics.rs`, next to `diagnose`.**
  `matched_by_source(records, filter) -> BTreeMap<String, usize>` takes the same
  two inputs as `diagnose` and returns plain data. `BTreeMap` gives a
  deterministic insertion order, so JSON consumers see a stable key order.

- **Computed once, passed as `Option` rather than always built and dropped.**
  The renderer gates on the funnel being carried, so on an ordinary run the map
  is still built (it is cheap) but never read; the gate lives at the boundary
  where the decision belongs, so `OutputRenderer::json` never has to reason
  about "should I include this" independently of the caller's rule.

- **One `source_info` helper, two call sites.** `json` and `render_diff_json`
  both built the `SourceInfo` inline, duplicating the `status` mapping. Both
  now call `source_info(&status, matched_map)`, so the shared field cannot
  diverge between the `usage` frame and the `diff` frame.

## Success criteria

- On an empty result, `sources[].matched == 0` for every source, `records`
  unchanged, `diagnostics` present.
- `sum(sources[].matched) == diagnostics.matched` for every filtered run,
  including partial matches where only one source is selected.
- A non-empty run without `--explain` produces a frame byte-identical to the
  pre-change output: no `matched`, no `diagnostics`.
- `watch --json` and `usage --json` remain byte-identical for every flag
  combination, including the cases that now carry `matched`.
- `diff --json` carries no `matched` key.

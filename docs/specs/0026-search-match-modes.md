---
id: 0026
title: "search — match modes: regex and fuzzy"
status: done
created: 2026-09-19
updated: 2026-09-25
triage: done
---

## Problem Statement

`search` answers "what did we say about X?" with one tool: literal substring
matching. That answers exact-term questions well, but three everyday recall
problems fail silently:

- **Structured patterns.** Error codes, UUIDs, hashes, timestamps, version
  strings — "find every message mentioning a 401 from the request command"
  means `search "401"`, which also matches any other 401 in any other context;
  "find `parse_duration` as a word, not inside `parse_durations`" has no
  answer at all.
- **Variants.** A decision referred to now as `window_filter`, later as
  `window filter`, needs three separate substring runs (and a third for
  `windowfilter`) before the corpus can be considered covered.
- **Loose recall.** "It was something about budget checks" — the actual text
  said `budget gate`, or the user only remembers the shape
  (`budget chek`). Substring requires the exact term; there is no way to say
  "roughly this".

Today the only escape is to run several searches and deduplicate by eye.
Spec 0010 parked exactly this gap: "**Regex, glob, fuzzy, and typo-tolerant
matching.** Substring only in this iteration."

## Solution

`search` grows a match-mode selector: `--match substring|regex|fuzzy`,
defaulting to `substring` — today's behaviour, byte for byte.

- **`regex`** interprets the query as a regular expression (Rust `regex`
  crate, RE2-style syntax). Alternation, word boundaries, quantifiers, and
  character classes answer the structured and variant questions in one run.
- **`fuzzy`** interprets the query as a subsequence to be scored (fzf/skim
  style): characters must appear in order, and hits are ranked by how well
  they match rather than how often. This answers the loose-recall question.

Everything else is shared: the same corpus, the same four-layer filter
vocabulary, the same TUI, the same three non-interactive output modes, the
same exit codes, the same cache. A mode changes how one message is matched
against the query and what ranking weighs first — nothing else.

## User Stories

1. As a developer, I want `--match regex` so that I can find structured
   values (`HTTP/\d \d{3}`, 40-char commit hashes) that a substring query
   cannot express.
2. As a developer, I want alternation in one query (`window_filter|window
   filter`), so that I cover naming variants without running search three
   times and merging by eye.
3. As a developer, I want word boundaries (`\bparse_duration\b`), so that a
   term search does not return every message that merely contains it as a
   prefix of a longer identifier.
4. As a developer, I want inline pattern flags (`(?i)`, `(?s)`) to work, so
   that pattern-level control does not require new CLI flags.
5. As a user, I want `--match fuzzy` so that "budget chek" style recall —
   wrong, missing, or approximate terms — still surfaces the right message.
6. As a user, I want fuzzy hits ranked by relevance score, so that the
   tightest match outranks a message that merely mentions the letters.
7. As a script author, I want a default-mode run to produce byte-identical
   output to today's, so that no consumer of `search --json` notices this
   change.
8. As a script author, I want fuzzy scores in the JSON payload only when a
   fuzzy search produced them, so that ordinary payloads gain nothing.
9. As a user, I want an invalid regex to fail loudly with the exact regex
   error and exit 1 before any source is read, so that a bad pattern is
   never mistaken for "no matches".
10. As a user, I want `--case-sensitive` to compose with every mode, so
   that case precision does not depend on the match mode.
11. As a user, I want `--role`, `--project`, `--model`, `--source`,
   `--since`/`--last` and `--explain` to work identically under every mode,
   so that mode and scope are orthogonal concerns.
12. As a user, I want the TUI header to name the active mode when it is not
   the default, so that a regex or fuzzy search never masquerades as a
   substring one.
13. As a user, I want the snippet centred on the actual match — a regex
   match span or a fuzzy match span — so that the excerpt still points at
   why the message hit.
14. As a user, I want cached and uncached runs of any mode to be
   byte-identical, so that the cache stays an invisible accelerator (spec
   0025's contract) for every mode.
15. As a user, I want two runs with the same arguments to produce identical
   bytes in every mode, so that output remains diffable and testable.
16. As a user, I want an empty result to stay exit 0 with `--explain`
   support in every mode, so that "no matches" remains a valid answer.
17. As a spreadsheet user, I want a `score` column in CSV only for fuzzy
   searches, so that substring/regex exports keep their exact column set.
18. As a user, I want the regex compiled once per run, so that scanning the
   corpus pays compilation once, not per message.
19. As a maintainer, I want all three matchers behind one internal seam, so
   that a future matcher (or a replacement fuzzy scorer) is one
   implementation, not a fork of the engine.

## Implementation Decisions

- **`--match <MODE>` on `search` only**, a clap `ValueEnum` with values
  `substring`, `regex`, `fuzzy`, defaulting to `substring` — the same
  pattern as `--group-by` and `--sort-by`. `substring` reproduces today's
  behaviour with no interpretation of the query.
- **The match mode lives in `SearchOptions`** (as `match_mode`), so the
  public engine signature stays `search(messages, &SearchOptions)` —
  callers and tests keep one seam. `SearchHit` gains `score: Option<i64>`,
  `Some` only for fuzzy hits.
- **One internal `Matcher` abstraction in the search module**, constructed
  once per run from `(mode, query, case_sensitive)`, exposing exactly what
  the engine needs: does this text match, the first match span (byte
  offsets), and a non-overlapping match count. Three implementations:
  - **substring** — today's fold/`find` logic, unchanged; its byte-level
    behaviour is frozen by existing tests and is not rewritten.
  - **regex** — `regex = "1"`, built with
    `RegexBuilder::case_insensitive(!case_sensitive)`. RE2 syntax:
    no lookaround, no backreferences — deliberately. A 57 MB corpus must
    not be exposed to catastrophic backtracking; linear-time matching is
    the safety property being bought. In-pattern flags (`(?i)`, `(?s)`)
    remain available.
  - **fuzzy** — `fuzzy-matcher = "0.3"` (`SkimMatcherV2`), which returns a
    score plus matched **character** indices — exactly the two things the
    engine needs. Default configuration is explicit, not smart-case:
    case-insensitive unless `--case-sensitive` maps to the matcher's
    respect-case setting.
- **Counting semantics per mode.** `matches` remains the non-overlapping
  occurrence count for substring (unchanged) and regex (`find_iter` count,
  leftmost-first, non-overlapping — the same contract as today's counting).
  For fuzzy, a message either fuzzy-matches once or not at all, so
  `matches` is honestly `1` and the relevance lives in `score`. A hit is
  never reported with both figures; output never renders an absent one as
  `0`.
- **Ranking primary key follows the mode.** substring/regex rank by match
  count descending (today's rule); fuzzy ranks by score descending. The
  existing tiebreakers follow unchanged: timestamp descending, then source,
  then session id — the total order stays deterministic in every mode.
- **Snippet generalises to spans.** Today's snippet takes the first
  occurrence's offset and the query's length; the span-based form takes a
  byte range instead — substring: the matched occurrence; regex: the first
  match span; fuzzy: the span covering the minimum through maximum matched
  character indices. The context window, newline flattening, and `…`
  markers behave exactly as before; the `context` flag applies to all
  modes.
- **The empty-query guard outranks every mode.** An empty pattern is legal
  in all three engines — and an empty regex matches everything, which
  would turn a forgotten argument into a full-corpus dump. The existing
  guard (empty after trim → no hits; `query must not be empty` at
  validation) applies before any mode is constructed.
- **The regex is compiled at argument-validation time**, before any Source
  is discovered or read, so a bad pattern fails in milliseconds with
  `invalid --match regex: <error>` — the same wording family as
  `invalid --last` — and exit 1. substring and fuzzy never fail
  validation.
- **Output byte-identity is mode-gated, not unconditional.** substring
  runs: byte-identical to today — no new JSON keys, no new CSV columns, no
  new TUI spans. Non-default modes add exactly three things, each only
  when earned: a `"match"` key in the JSON payload carrying the mode name
  (absent under substring); `score` on fuzzy JSON hits and a `score` CSV
  column (absent otherwise, the same conditional-column rule as compare's
  `budget_state`); and a `match:regex` / `match:fuzzy` span in the TUI
  header line, with the hit-table column header switching from the match
  count to the score in fuzzy mode.
- **The cache is untouched.** Matching happens after extraction; spec
  0025's fingerprint index neither knows nor cares about match modes. The
  cold/warm byte-identity guards extend to all three modes rather than
  gaining mode-specific cache logic.
- **Exit codes and empty results are unchanged.** No matches is still exit
  0 in every mode — a valid answer — with `--explain` describing the
  filters as before; the mode never appears in diagnostics because the
  funnel counts messages, not matches.
- **The searchable corpus is unchanged.** `toolResult`/`tool_use`/`patch`/
  `image` parts stay excluded (spec 0010); the modes change matching, not
  what is searched.
- **Dependencies land with this feature, recorded in a new ADR**: `regex`
  for RE2 linear-time matching (no viable std alternative), and
  `fuzzy-matcher` for fzf-style scoring, accepting that its last release
  was 2020 — a pure algorithm crate whose risk is contained by the
  `Matcher` seam (swapping in a maintained scorer is one implementation).
  Both are compile-time additions to an already dependency-justified
  surface (`reqwest`, `rusqlite` bundled); neither is optional.

## Testing Decisions

- **The engine is the primary seam**: unit tests exercise
  `search(messages, &SearchOptions)` only, never internal matcher variants.
  Coverage per mode: match/no-match; counting (alternation counted per
  occurrence, non-overlap, multibyte); case sensitivity × mode; snippet
  span correctness (match at start/end/middle, spanning newlines); ranking
  (regex by count, fuzzy by score, with the deterministic-tiebreak guard
  that two runs produce identical order); the empty-query guard; and that
  `score` is `Some` exactly for fuzzy hits.
- **A non-vacuous ranking guard**, in the pattern of spec 0024's
  mixed-dimension test: a fixture where the highest-scoring fuzzy hit is
  *not* the one with the most textual matches, asserting fuzzy actually
  ranks by score — so the score key cannot pass while being ignored.
- **Byte-identity guards carry the compat promise**: existing default-mode
  assertions in the search integration suite stay untouched and green;
  plus one explicit test that a substring run's JSON payload carries no
  `match` key and no `score` fields — the mechanical form of "byte-identical
  to today".
- **Integration tests (prior art: `tests/search.rs`)** run the binary
  against the fixtures: `--match regex` end to end (hits, counts,
  snippets), `--match fuzzy` (scores present, score-ordered), each
  composing with `--role`, `--project`, `--since`; `--case-sensitive` per
  mode; invalid regex exit 1 with the exact stderr wording and no source
  warnings; `--json`/`--csv`/`--text` shapes per mode (CSV gains its
  `score` column only under fuzzy); and the cache cold/warm equality test
  from 0025 re-run per non-default mode.
- **TUI state/render tests** assert the header mode span appears only for
  non-default modes and the score column header switches only in fuzzy
  mode, reusing the existing pure-function seams; the list+detail state
  machine tests stay untouched.
- **What makes a good test here** is unchanged from the repo's standing
  rule: assert external behaviour (stdout bytes, stderr bytes, exit
  codes), never internal representation; prefer the highest seam that can
  catch the regression, with engine-level tests for semantics and
  binary-level tests for contracts.

## Out of Scope

- **Typo-tolerant / edit-distance matching.** Fuzzy here scores an
  in-order subsequence (fzf semantics); it does not tolerate wrong or
  substituted characters in the middle of a word. Spec 0010's parked
  "typo-tolerant" item remains out of scope.
- **Glob matching.** Globs address names, not text content; the corpus is
  message text.
- **Semantic or embedding-based search.** Unchanged from 0010: no vector
  store, no model calls.
- **Per-mode flags.** No `--fixed-strings` (substring *is* the default),
  no `--multiline` (use the inline `(?s)` flag), no fuzzy-score knobs
  (the scorer ships its default configuration).
- **Regex/fuzzy for filter predicates.** `--project` and `--model` keep
  their existing substring semantics; modes apply to message text only.
- **Modes on other commands.** `watch`, `usage`, `report` et al. do not
  take text queries; `search` owns interactive text matching.
- **Query-result caching.** Spec 0025 memoizes extraction, not queries;
  two different queries over a warm cache still both scan the corpus.
- **Smart-case.** Case behaviour stays explicit: insensitive unless
  `--case-sensitive`, in every mode.

## Further Notes

- `fuzzy-matcher` has been unreleased since 2020 (0.3.7; ~31M downloads,
  one small runtime dependency, MIT). The ADR records the acceptance
  rationale and the alternative (`nucleo-matcher`, maintained, from the
  Helix editor): the swap is one `Matcher` implementation away if scoring
  quality or maintenance ever forces it.
- The regex crate guarantees linear-time matching, which is why the
  RE2-style feature gap (no lookaround/backrefs) is a feature for this
  use: the matcher runs over every message of a 57 MB corpus on the same
  machine where `search` already answers in ~0.1 s warm (spec 0025
  baseline).
- Per spec 0025, message extraction is memoized; steady-state mode runs
  pay fingerprint reads + index deserialization + matching only. The
  changelog entry for this feature must record measured mode overhead
  against that baseline, per the 0025 pattern of measured-not-promised
  numbers.
- Spec 0010's "Regex, glob, fuzzy, and typo-tolerant matching" Out-of-Scope
  entry gains a pointer here for the two modes delivered (same pattern as
  0017→0018 and 0010→0025); glob and typo-tolerance stay parked there.
- Frontmatter note: spec 0010's own `status` field predates the done-marking
  convention and reads `ready-for-agent`; this spec adopts the current
  convention from birth.

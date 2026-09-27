# Search matches by mode; the default mode stays byte-identical

`search` could answer one question: "which messages contain this exact
substring?" Spec 0010 shipped that deliberately and parked the rest —
"Regex, glob, fuzzy, and typo-tolerant matching. Substring only in this
iteration." Since then the recall gaps became daily friction: alternations
need three separate runs (`window_filter`, `window filter`, `windowfilter`),
word-boundary questions have no answer at all, and a misspelled recollection
(`budget chek`) simply misses. The escape hatch was running several searches
and deduplicating by eye.

Search gains one selector, `--match substring|regex|fuzzy`, defaulting to
`substring` — today's behaviour, byte for byte. `regex` interprets the query
as an RE2-style pattern (alternation, boundaries, quantifiers, classes).
`fuzzy` scores every message fzf-style and ranks by that score, so
`regrx` finds `regex`. The mode lives in `SearchOptions`, so the public engine
signature stays `search(messages, &SearchOptions)` and every existing test and
caller keeps its seam.

Three decisions are load-bearing.

- **The default is not a new mode — it is the old code.** substring's fold
  and `find` logic is untouched — the substring arm of the `Matcher` seam only
  delegates to the original `count_matches`/`find_span` helpers, so the new
  implementations are entered by the non-default modes alone. This is why
  "substring reproduces today" is a fact and not a hope, and why the
  byte-identity guards (no `match` key, no `score` field, no new CSV column,
  no new TUI span under substring) pass unchanged. Non-default
  modes may add output only when earned: the `match` echo key, the `score`
  field and column, the TUI header span and the score column header — each
  gated on the mode that produced it, the same conditional-column rule
  ADR-free compare uses for `budget_state`.
- **Counting stays honest in every mode.** substring and regex both count
  non-overlapping occurrences (`find_iter` is the same contract as today's
  `find`-counting loop); fuzzy is one-or-zero by nature, so `matches` is
  honestly `1` and the relevance lives in `score: Option<i64>` — `Some` only
  for fuzzy hits, skipped on the wire otherwise, and never rendered as a
  fabricated `0`. Ranking's primary key follows the mode (count desc /
  score desc) with today's tiebreakers untouched, so the order stays total
  and deterministic in every mode.
- **Regex is RE2-on-purpose, and invalid patterns fail before any source is
  read.** The `regex` crate is linear-time — no catastrophic backtracking on
  a corpus that reaches 57 MB — at the cost of no lookaround and no
  backreferences; in-pattern flags (`(?i)`, `(?s)`) remain available.
  Validation compiles the pattern through the engine's own `compile_pattern`
  — the exact constructor the search will use — so a bad pattern exits 1 with
  `invalid --match regex: <error>` in milliseconds, before any source is read,
  never masquerading as "no matches" after a full corpus load; the engine
  then builds its matcher once per run and reuses it for every message. The
  empty-query
  guard outranks every mode: an empty regex would legally match everything,
  which turns a forgotten argument into a full-corpus dump, so the existing
  validation fires before any matcher is constructed.

Case sensitivity stays an explicit flag in every mode. Smart-case (fzf's
implicit "lowercase means insensitive") was rejected: it makes results depend
on the query's own case in a way no other llmhelper flag mirrors, and the
repo's rule is that surprising defaults live behind explicit flags —
`--case-sensitive` already exists and composes with all three modes.

`fuzzy-matcher` (0.3, last released 2020) is accepted despite its age: it is a
pure algorithm crate with no network, filesystem, or unsafe surface, and the
`Matcher` seam contains the risk — swapping in a maintained scorer is one
implementation, not a rewrite. Its scores are fzf-style integers
(contiguous-run bonuses dominate), which makes cross-mode score comparison
meaningless by design; nothing outside a fuzzy run reads a score.

The cache (ADR 0005) is untouched: matching happens after extraction, and the
fingerprint index neither knows nor cares about modes. The cold/warm
byte-identity guards re-run per non-default mode rather than gaining
mode-specific cache logic. Exit codes are unchanged — no matches is still
exit 0 in every mode — and the mode never appears in diagnostics, because the
funnel counts messages, not matches. The searchable corpus is unchanged too:
the modes change matching, not what is searched.

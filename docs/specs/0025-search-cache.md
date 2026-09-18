---
id: 0025
title: "search — incremental message cache"
status: done
created: 2026-09-15
updated: 2026-09-15
triage: done
---
## Problem Statement

Every `search` run re-extracts the entire message corpus from disk: it parses
all of Claude's transcript JSONL, all of OMP's session JSONL (57 MB here), and
scans both OpenCode and Kilo SQLite databases, materialising ~15k `Message`
structs before the query engine ever runs. Measured on this machine: `search`
cold-runs in ~1.8 s while the record-level commands (`usage`, `sessions`) run
in ~0.13 s — a 13× gap, and essentially all of it is re-reading files that
have not changed since the last run.

The cost lands on every invocation: each TUI `r` refresh in `search` re-scans
the corpus, and `export --messages` (which shares the corpus) pays the same
tax. The corpus is overwhelmingly static — yesterday's transcripts do not
change — yet every run pays full price for them.

Spec 0010 parked exactly this: "**Incremental indexing.** The corpus is scanned
on each run; caching is a future concern." This spec is that concern.

## Solution

Memoize message extraction **per source file**. On each run, every file's
identity is checked against a small fingerprint (path, size, mtime, inode);
files whose fingerprint matches the last extraction have their messages
reused from an on-disk cache, and only new or modified files are re-parsed.
A run that changes nothing re-reads nothing.

The cache is a pure accelerator, invisible in the results:

- **Byte-identical results.** A cached run and an uncached run of the same
  query produce the same bytes. This is asserted by tests, the same way
  `watch --json ≡ usage --json` is. The corollary, and the part worth being
  strict about: plain `search --json` gains **no** `cache` key, so the two
  differ in nothing — not even a `"cache": null`. Non-interactive search
  cache statistics appear only under `--explain`; the interactive search
  header is the explicit exception described under Observability below.
- **Fail-open.** Any anomaly — corrupted cache line, unreadable cache
  directory, fingerprint that cannot be read — falls back to a full
  re-extraction of the affected file. The cache can make a run slower; it can
  never make it wrong or empty.
- **Opt-out.** `--no-cache` restores today's behaviour byte for byte; a
  `--cache-dir` override (and a config key) keeps tests and exotic setups
  isolated.

## User Stories

1. As a user, I want a second `search` run to skip re-parsing transcript files
   that have not changed, so that the query answers in a fraction of the
   first run's time.
2. As a user, I want messages from a file I just appended to appear in the
   next search, so that the cache never shows me a stale corpus.
3. As a user, I want the cache validated by file identity (size, mtime,
   inode), so that an edited file is re-extracted even if its length happens
   to stay the same.
4. As a user, I want a corrupted or truncated cache file to be discarded and
   rebuilt rather than producing errors or missing messages, so that a broken
   cache costs one slow run, never a wrong answer.
5. As a user, I want cached and uncached searches to produce identical
   output, so that the cache changes nothing about what I see.
6. As a user, I want `--no-cache`, so that I can bypass the cache when I
   distrust it or am measuring baseline performance.
7. As a user, I want `--cache-dir`, so that tests and sandboxed environments
   can isolate the cache from my real one.
8. As a user, I want the cache to live under my system's cache directory, so
   that it never pollutes config or data directories and respects platform
   conventions.
9. As a user, I want `export --messages` to benefit from the same cache, so
   that transcript dumps stop re-scanning unchanged files too.
10. As a user, I want the search TUI header to say how much of the corpus was
    reused, so that the cache is observable rather than magic.
11. As a user, I want `--explain` to report cache provenance alongside the
    funnel, so that "why is this fast/slow" is answerable in the same breath
    as "why is this empty".
12. As a user, I want two concurrent searches not to corrupt each other's
    cache, so that parallel invocations are safe.
13. As a user, I want the cache to be keyed by the actual source directories
    in use, so that pointing `--claude-dir` at a fixture tree never reuses
    messages from my real corpus (or vice versa).
14. As a user, I want a source that fails to load to behave exactly as it
    does today, so that the cache adds no new failure modes to degraded
    sources.
15. As a user, I want record-level commands (`usage`, `diff`, `sessions`,
    `report`, `watch`, `trend`, `compare`) completely unaffected, so that
    this change cannot perturb the fast paths.
16. As a user, I want the cache disabled automatically when the cache
    directory cannot be created or written, so that an unwritable home never
    breaks a search.
17. As a user, I want a config key to relocate the cache, so that users with
    read-only homes or shared machines can point it at a writable path.
18. As a maintainer, I want the cache confined to a new module and per-file
    call sites inside the adapters, so that dispatch stays by `name()` through
    the same registry and no command-level call site learns that a cache
    exists (ADR 0002). *Amended in review:* the registry itself does change —
    it fans the per-Source message loads out across threads and relays each
    Source's stats and flush — because memoizing every file read makes those
    loads independent and there is no reason to keep serializing them.
19. As a maintainer, I want the TUI full-reread contract (ADR 0003) to hold,
    so that `r` keeps meaning "read everything again" — just cheaply.
20. As a maintainer, I want a byte-equality guard test between cached and
    uncached runs, so that any drift in extraction order or shape fails CI
    instead of shipping.

## Implementation Decisions

### Per-file memoization, not per-source

The cache granularity is **one file** (a Claude transcript JSONL, an OMP
session JSONL, a SQLite database file), not one source. Re-extracting all of
OMP because one session gained a line would repeat the problem at a smaller
scale. Each adapter already walks a file inventory inside its
`load_messages()`; that walk gains one decision per file:

```
fingerprint(file) matches cache entry -> reuse cached messages
otherwise                             -> extract fresh, update cache
```

SQLite sources have a one-file inventory (the database), so their granularity
is the whole DB — correct, if occasionally conservative.

### The fingerprint

A file's identity is `(canonical path, size, mtime in nanoseconds, inode)`.
This is the same class of key `make` uses. Two honest caveats, both recorded
rather than hidden:

- A write that changes content but preserves size **and** mtime to the
  nanosecond would be missed. Real editors and agent tools do not do this;
  the risk is accepted and documented, matching mainstream build tools.
- The inode component is unix-only (`cfg(unix)`, like the existing `libc`
  dependency). On other platforms the key degrades to path + size + mtime.

Canonical paths in the key are what make story 13 true: a fixture tree and a
real corpus can never collide.

### One new module, wired inside the adapters

A new pure-ish module (no clock, no network; only cache-directory I/O)
exposes:

- `FileFingerprint` — captured from `std::fs::metadata`.
- A cache handle opened once per run from the resolved cache directory:
  load the on-disk index, serve `reuse_or_extract(fingerprint, extract)`
  per file, and rewrite the index atomically iff anything was extracted or
  the load's stale-entry prune dropped an entry (see the 2026-09-14
  correction below).
- `CacheStats` — Source count, files seen / reused / extracted, messages
  reused / extracted.

Each adapter holds an `Option<cache>` handle set at construction time (the
registry-building site in the binary). `None` means "cache disabled" and
degrades to today's code path.

**Correction, 2026-09-13 (review follow-up).** The first draft of this section
claimed the `Source` trait, `Registry`, and `load_messages_all` would stay
untouched. They do not, and the review settled on keeping it that way rather
than reverting it, for one reason worth recording: once every file read is
memoized, the four Sources' `load_messages()` calls are independent, and
loading them sequentially leaves the cold-cache run — the one this spec exists
to fix — bound by their sum rather than their max. So `load_messages_all` now
spawns one scoped thread per Source and joins in registry order, and the trait
gained two pass-through methods (`flush_cache`, `cache_stats`) whose defaults
are no-ops on `None`. What survives from ADR 0002's shape is the part that
mattered: dispatch is still by `name()` through the same map, no command
below the registry knows a cache exists, and a Source that panics yields an
error status exactly as a sequential failure did. ADR 0003 (full reread on
live refresh) is likewise untouched: a refresh still walks every file; the
cache just makes the walk cheap, and the load cheaper. This decision warrants
a new ADR (0005) recording the fingerprint contract, the fail-open rule, and
the parallel-load exception.

Adapter-level post-processing — OMP's single-pass record/message collection,
the OpenCode/Kilo cross-database session merge and dedup, role filtering —
runs **after** per-file cache retrieval on the merged sets, exactly as today.
The cache stores raw per-file messages only; no derived state is ever cached.

### Index format and atomicity

One cache file per source: a JSON Lines document under the cache directory,
one object per cached file holding its fingerprint and that file's messages.
Per-line parse means a torn or corrupted tail costs only the lines after it —
those files re-extract and the index self-heals on the next write.

Writes go to a temporary file in the same directory followed by an atomic
rename. Concurrent invocations therefore see either the old or the new index
in full; a lost update merely costs a re-extraction later (story 12). The
cache directory is created on demand; if creation or writing fails, the run
proceeds cacheless and silently (results are identical — only speed differs;
story 16). A `cache` miss is never an error surfaced to the user.

Serialization reuses the existing `Message` serde impls (chrono timestamps
included). No new dependencies; NDJSON is parsed with the serde_json stream
deserializer already available.

### CLI and config surface

- `search` and `export` gain `--no-cache`, `--cache-dir <path>`, and
  `--refresh-cache`.
- A `[cache] dir` config key overrides the default
  (`dirs::cache_dir()/llmhelper`); the flag overrides the config. This
  follows the existing source-path override pattern (`--claude-dir` etc.).
- The flags live on the message-reading commands only. Record-level commands
  neither accept nor need them (story 15).
- `--refresh-cache` (added after review) deletes each Source's index before
  the handle opens, so the run re-extracts everything and rewrites the index
  from scratch. The fingerprint check already covers every case a user can
  hit — a newline changes size, a rewrite changes mtime — so this is a big
  hammer for the case where a filesystem lies about both. It is deliberately
  *not* a standalone "empty the cache" surface: it runs one load and
  rebuilds, it does not leave the cache empty.

**Correction, 2026-09-14 (review follow-up).** Three gaps in the first
landing, all closed:

1. **Stale entries are pruned, not merely never matched.** The Out-of-Scope
   wording below ("simply never matched and are rewritten out on the next
   extraction") implied the index shrank on its own; in fact nothing rewrote
   a fully cached run, so a deleted file's entry lived forever. Now each
   Source's load announces itself with `begin_load`, which records every path
   the walk reaches (hit or miss), and `flush` drops index entries whose path
   was never reached — rewriting even an otherwise all-hits index. A load
   that never announced (a record-only run, or one that errored before
   walking) still prunes nothing: an empty visited-set is "unknown", not
   "corpus empty", so a warm index survives a run that walked no files. An
   *announced* walk over an empty corpus does prune its own Source's index to
   nothing — and only its own, since each Source owns an index file.
2. **`--refresh-cache` survives an undeletable index.** Deleting the index
   file was already best-effort, but when deletion fails (a read-only cache
   directory), the handle then loaded the stored entries it was told to
   replace — exactly the one case where a stale entry is user-contradicted.
   The refresh branch now opens the handle with stored entries ignored
   (`cache_handle_fresh`), so the run re-extracts everything regardless; the
   rewrite itself is retried on a later run, as with any write failure.
3. **Per-run cache statistics.** The counters accumulate across the TUI's
   `r` refreshes because the handle outlives the run, so a second load
   reported the sum since process start. `begin_load` zeroes the counters at
   the top of each Source's `load_messages`; the `--explain` line and the
   TUI header describe this run's loads only.

### Observability without breaking byte-identity

`CacheStats` surfaces in exactly two places:

- The `search` TUI header gains one segment on its filter line
  (`cache: 15113 reused, 0 re-read`) — visible on every cached run,
  cold or warm, since the TUI is a human surface. It is absent under
  `--no-cache`, where there is no cache whose provenance could be reported.
- `--explain`: a `cache` object in the JSON payload, and one stderr line in
  table/CSV mode (the 0020 asymmetry — structured data in JSON, prose on
  stderr, body untouched). In JSON mode the payload is the whole story, so
  the stderr line is not printed there as well.

Normal `--json` output **gains no cache keys**: the byte-identity contract
between cached and uncached runs is load-bearing and a stats field would
break it. This mirrors how `diagnostics` is gated (0020).

### Empty and error behaviour unchanged

A cache-disabled run, an empty corpus, a degraded source, and a zero-match
query behave exactly as they do today — same exit codes, same stderr
conventions, same `MessageStatus` reporting. Nothing in the error-handling
table of spec 0010 changes.

## User Interface

Search TUI header, before:

    case-insensitive · role: all · <active filters>

after (second run, warm cache):

    case-insensitive · role: all · <active filters> · cache: 15113 reused, 0 re-read

`--explain` (stderr, table/CSV mode):

    cache: 4 sources, 82 files reused, 1 re-read (15,113 messages reused, 12 extracted)

JSON `--explain` payload gains a `cache` object with the same counts. All
figures describe **this run's** loads — reused means served from cache,
re-read means fingerprint-missed and extracted fresh.

## Error Handling

| Condition | Behaviour |
|---|---|
| Cache dir missing | created on demand |
| Cache dir not writable | run proceeds cacheless, silently |
| Cache index corrupted mid-file | affected lines discarded; those files re-extract; index self-heals on next write |
| Fingerprint unreadable for a source file | that file re-extracts |
| Source file missing/absent | unchanged — existing `SourceError::Absent` semantics |
| Two concurrent runs | atomic rename; worst case a wasted re-extraction |
| `--cache-dir` points at a file, not a directory | cache disabled for the run, silent |

## Testing Decisions

- **Byte-identity is the headline guard**, prior art: `tests/watch.rs`'
  `watch --json ≡ usage --json`. `tests/search.rs` (or the cache's own
  integration file) runs the same fixture-backed `search --json` twice —
  once cold (`--no-cache`), once warm (shared temp cache dir) — and asserts
  identical stdout. Same pair for `export --messages`.
- **Freshness is mutation-tested**: run once to warm, append a line to a
  fixture transcript (size changes), run again, and assert the new message
  appears. Appending changes size, so no mtime-setting dependency is needed —
  std cannot set mtimes and no dev-dependency is added for this.
- **Isolation**: every cache test points `--cache-dir` at a per-test temp
  directory. The spec 0018/0019 trap (real user data leaking into fixtures —
  and now the reverse, fixture data leaking into the real cache) is recorded
  and guarded.
- **Unit tests on the cache module** (no adapters, no fixtures): fingerprint
  mismatch on size change, mismatch on mtime change, corrupted-line
  self-heal, non-writable directory degrade, empty-corpus behaviour, index
  shape stability. These gain the 2026-09-14 correction's cases: stale-entry
  pruning on an announced walk, no pruning without one, and the per-load
  counter reset.
- **The pruning and refresh guarantees are integration-tested against the
  real binary** (the cache integration file): deleting a fixture copy's
  transcript and re-running drops that entry from the on-disk index while the
  surviving entries stay; a Source pointed at an empty directory prunes only
  its own index; and `--refresh-cache` with the cache directory made
  read-only reports `files reused` of 0 even though the index file could not
  be deleted. Each was observed failing with the corresponding behaviour
  disabled before the fix made it pass.
- **Adapter tests** already cover extraction shape; they gain one case each:
  with a cache handle, a second extraction returns the same messages.
- **Stats surfaces**: TUI header segment asserted via the existing pure
  header-line function tests; `--explain` cache object asserted in the
  explain integration tests; normal-JSON-gains-no-key asserted as a
  byte-level test.
- Record-level suites get **zero new assertions** because they get zero
  changes — their existing byte-stability tests are the guard.

## Out of Scope

- **Record-level caching.** Record commands run in ~0.13 s; there is nothing
  to win and a fast-moving cache to maintain.
- **Line-level incremental parsing** (re-parsing only the appended tail of a
  JSONL). File-level granularity is the cut; tail-parsing would need
  per-file offsets, partial-line handling, and a much harder corruption
  story, for seconds already saved.
- **A full-text index** (inverted index, tokenization, ranking). Search
  stays substring matching; the cache memoizes extraction, not queries.
- **Query-result caching.** Two different queries over a warm cache still
  both scan the in-memory corpus; only disk I/O is memoized.
- **Cache eviction/TTL.** Stale entries for deleted files are simply never
  matched and are rewritten out on the next extraction; a size cap and
  eviction policy can come later if cache weight ever matters.
- **Windows inode parity.** Non-unix platforms use path + size + mtime; the
  weaker key is accepted there, documented, not engineered around.

## Further Notes

- Measured baseline motivating this spec (this machine, release build):
  `search` 1.79 s (`--source kilo` 1.34, `--source omp` 0.73, claude 0.46,
  opencode 0.48); `usage`/`sessions` 0.13 s. Corpus: ~15.1k messages, 57 MB
  OMP JSONL + 8.8 MB Claude JSONL + two SQLite DBs.
- The expected steady-state cost after this spec: read N fingerprints +
  parse the NDJSON index (in-memory message deserialization only, no source
  parsing). Whether that lands at 0.1–0.3 s is a measured outcome, not a
  promise — the spec's promise is byte-identity and freshness, with speed as
  the motivated consequence.
- Specs touched on landing: 0010's "Incremental indexing" Out-of-Scope entry
  gains a pointer here (same pattern as 0017→0018 and 0014→0016).

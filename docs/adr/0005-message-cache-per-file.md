# Message extraction is memoized per file; records still re-read

`search` (and `export --messages`) re-extract the entire message corpus from
disk on every run: Claude transcript JSONL, OMP session JSONL (57 MB here), and
both SQLite databases — materialising ~15k `Message` structs before the query
engine runs. Measured baseline: `search` cold-runs in ~1.8 s while the
record-level commands run in ~0.13 s, and a TUI refresh repeats the cost.

We memoize message extraction **per source file**. A file's identity is a
`make`-class fingerprint — canonical path, size, nanosecond mtime, inode — and a
file whose fingerprint is unchanged has its messages served from an on-disk
index instead of being parsed. Only new or modified files re-extract.

ADR 0003 deferred exactly this ("do not add caching preemptively") for the
live-refresh TUI, on the grounds that it was fast enough and caching introduces
stale-cache edge cases. Two facts changed the calculus:

- **Profiling showed the need.** Search was an order of magnitude slower than
  the other reads, and the cost was pure I/O over files that had not changed.
  That is the threshold 0003 named. The cache is pointed at the *slowest*
  path, not added preemptively across the tool.
- **Record reads are untouched.** `usage`, `diff`, `trend`, `compare`, `watch`
  and `report` still read everything from disk every time, byte for byte 
  unchanged. The cache exists only behind `load_messages`, so ADR 0003's
  "full re-read each tick" remains true for the record side, and the message
  side's refresh is still a full walk — it just skips parsing unchanged files.

The load-bearing contract is **transparency**: a cached run and an uncached run
produce the same non-interactive output, byte for byte. Search `CacheStats`
figures live behind `--explain` — a stderr line in text mode, a `cache` object
in the JSON payload — with an explicit human-facing exception: the search TUI
header always shows reuse counts when caching is enabled. Normal
`search --json` and `export --messages` are byte-identical
whether the cache is warm, cold, or off. This is pinned by integration tests
that diff the raw stdout of a warm run against a `--no-cache` run and assert
equality, in both formats.

A second exception, added in review: `Registry::load_messages_all` fans the
per-Source message loads out across scoped threads and joins them in registry
order, instead of loading Source by Source. Memoizing every file read is what
makes the Sources independent — there is no shared state left for the load
order to matter — so serializing them would leave the cold-cache run (the very
run this ADR is about) bound by the sum of four loads rather than their max.
The registry still dispatches by `name()` through the same map, still reports
one `MessageStatus` per Source in the same order, and still turns a panicking
Source into an error status rather than aborting the run. ADR 0002's rule —
capability added without a new dispatch mechanism — survives; the claim that
the registry is untouched does not, and is retracted here rather than quietly
left standing.

The decision that makes this safe to leave on:

- **Fail-open.** Any anomaly — a corrupted index line, an unreadable cache
  directory, a fingerprint that cannot be taken — degrades to a full
  re-extraction of the affected file(s). The cache can make a run slower; it
  can never make it wrong or empty. A broken cache is therefore silent by
  design: results are identical, only the speed differs.
- **Fingerprint honesty.** Path + size + mtime-to-the-nanosecond + inode is
  the same key mainstream build tools use. A write that preserved all four
  would be missed; real editors and agent tools do not do this, and the risk
  is recorded rather than engineered away (a content hash would cost reading
  every file anyway, defeating the cache).
- **Cache is never a source of truth.** The index is a cache, not a store:
  delete files under `~/.cache/llmhelper/` and the worst outcome is one slow
  run. No user data lives there that exists nowhere else.

One deliberate asymmetry: for OMP, the message half of a file may be served
from the cache while the record half is always computed from disk. Coupling the
record half to the cache would make `usage`'s numbers depend on a cache that
exists only to speed up search — a wider blast radius for no benefit.

Corollaries for maintainers: never add a field to `Message` without bumping the
cache layout (a stale shape would silently deserialize wrong), and never cache
the *derived* state — the cross-database session merge and the filter both run
after retrieval, exactly as they do on a full read.
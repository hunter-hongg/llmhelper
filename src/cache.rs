//! Incremental message cache.
//!
//! Every `search` (and `export --messages`) run re-extracts the whole message
//! corpus from disk: Claude transcript JSONL, OMP session JSONL, and both
//! SQLite databases. That is ~1.8 s of I/O and 15k deserialisations on a
//! typical machine, and it is repeated on every TUI `r` refresh — even though
//! the corpus is overwhelmingly static (yesterday's transcripts do not change).
//!
//! This module memoizes extraction **per source file**. A file's identity is a
//! `make`-class fingerprint — canonical path, size, nanosecond mtime, inode —
//! and a file whose fingerprint is unchanged has its messages reused from an
//! on-disk index instead of being parsed again. Only new or modified files are
//! re-extracted.
//!
//! Two contracts make the cache safe to leave on:
//!
//! * **Byte-identical results.** A cached run and an uncached run of the same
//!   query produce the same bytes. The cache is a pure accelerator; it never
//!   changes what is returned. (Asserted in `tests/cache.rs`.)
//! * **Fail-open.** Any anomaly — a corrupted index line, an unreadable cache
//!   directory, a fingerprint that cannot be taken — falls back to a full
//!   re-extraction of the affected file. A cache can make a run slower; it can
//!   never make it wrong or empty.
//!
//! The module reads no clock and holds no global state: the only I/O is the
//! cache directory itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::message::Message;
use crate::source::SourceError;

/// Identity of one source file.
///
/// `(canonical path, size, mtime in nanoseconds, inode)`. Two honest caveats,
/// recorded rather than hidden: a write that changes content while preserving
/// both size and nanosecond mtime would be missed (accepted, matching mainstream
/// build tools); and the inode component is unix-only, so other platforms
/// degrade to path + size + mtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    /// Canonical (symlink-resolved) absolute path. Part of the key so a fixture
    /// tree and a real corpus can never collide.
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
    #[serde(default)]
    pub inode: Option<u64>,
}

impl FileFingerprint {
    /// Capture a file's identity. `None` when the file cannot be stat-ed, which
    /// the caller treats as a cache miss (fail-open).
    pub fn capture(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let canonical = std::fs::canonicalize(path)
            .ok()
            .unwrap_or_else(|| path.to_path_buf());
        Some(Self {
            path: canonical,
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            inode: inode(&meta),
        })
    }
}

#[cfg(unix)]
fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    // `mtime()` is seconds and `mtime_nsec()` the sub-second part; folding both
    // into one i64 keeps the key comparison a single integer.
    meta.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(meta.mtime_nsec())
}

#[cfg(not(unix))]
fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ino())
}

#[cfg(not(unix))]
fn inode(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

/// Per-run counts, surfaced by the search TUI header and `--explain`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Message-reading Sources that contributed a cache handle. Summed by the
    /// registry, which is the only place that knows how many Sources there are;
    /// the per-file counters below are summed from those Sources.
    pub sources: usize,
    pub files_seen: usize,
    pub files_reused: usize,
    pub files_extracted: usize,
    pub messages_reused: usize,
    pub messages_extracted: usize,
}

impl CacheStats {
    /// Whether anything at all was observed. A run with no message sources
    /// (or a disabled cache) reports `None` rather than a line of zeroes.
    pub fn is_observed(&self) -> bool {
        self.files_seen > 0
    }

    /// The `search` TUI header's cache segment: `cache: 15113 reused, 0 re-read`.
    ///
    /// Deliberately terse and ungrouped — it rides the header line beside the
    /// active filters, where a second line would cost a hit-row of screen space
    /// for a number a human only glances at. `--explain` carries the full,
    /// grouped figures.
    pub fn header_line(&self) -> String {
        format!(
            "cache: {} reused, {} re-read",
            self.messages_reused, self.messages_extracted
        )
    }

    /// The `--explain` stderr line:
    /// `cache: 4 sources, 82 files reused, 1 re-read (15,113 messages reused, 12 extracted)`.
    ///
    /// Both halves of each pair appear even when one is zero: `0 reused` is the
    /// signal that the cache is cold, which a reader who only saw `82 files`
    /// would miss.
    pub fn explain_line(&self) -> String {
        format!(
            "cache: {} {}, {} files reused, {} re-read ({} messages reused, {} extracted)",
            self.sources,
            if self.sources == 1 {
                "source"
            } else {
                "sources"
            },
            group_digits(self.files_reused),
            group_digits(self.files_extracted),
            group_digits(self.messages_reused),
            group_digits(self.messages_extracted),
        )
    }
}

/// Render a count with `,` thousands separators (`15113` -> `15,113`).
///
/// Hand-rolled because `std` has no grouping formatter and the spec's `--explain`
/// wording shows grouped figures; adding a dependency for this would be absurd.
fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One cached file: its fingerprint plus the messages extracted from it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedFile {
    #[serde(flatten)]
    fingerprint: FileFingerprint,
    messages: Vec<Message>,
}

/// A run-scoped handle over one source's on-disk cache index.
///
/// Opened once per run from the resolved cache directory, consulted once per
/// file, and flushed once at the end (only if something was extracted or the
/// load's stale-entry prune dropped an index line).
pub struct MessageCache {
    /// `None` when caching is disabled or the directory is unusable. Every
    /// method degrades to "just extract" in that case.
    dir: Option<PathBuf>,
    /// Cache index file name for this source (e.g. `omp.ndjson`).
    file_name: String,
    entries: HashMap<PathBuf, CachedFile>,
    /// Paths this load has visited, for index pruning: entries whose path is
    /// not in here when the index is flushed describe files that no longer
    /// exist and are rewritten out. Cleared by `begin_load` (spec 0025's
    /// stale-entry guarantee).
    seen_paths: HashSet<PathBuf>,
    /// Set by `begin_load`, so `flush` knows this load actually walked the
    /// corpus. Without it, a flush from a load that visited nothing would read
    /// the empty `seen_paths` as "the corpus is empty" and drop every entry.
    corpus_walked: bool,
    stats: CacheStats,
    dirty: bool,
}

impl MessageCache {
    /// Open (or initialize) the cache index for `name` under `dir`.
    ///
    /// Failure to create or write the directory does not fail the run: the
    /// handle degrades to a pass-through that extracts everything. When
    /// `load_from_disk` is false the stored entries are ignored, so a handle
    /// that will rewrite the index from scratch cannot reuse a stale entry.
    pub fn open(dir: &Path, name: &str, load_from_disk: bool) -> Self {
        let file_name = format!("{}.ndjson", sanitize_name(name));
        let usable = std::fs::create_dir_all(dir).is_ok() && dir.is_dir();
        let mut this = Self {
            dir: usable.then(|| dir.to_path_buf()),
            entries: HashMap::new(),
            seen_paths: HashSet::new(),
            corpus_walked: false,
            stats: CacheStats::default(),
            dirty: false,
            file_name,
        };
        if usable && load_from_disk {
            this.load_index();
        }
        this
    }

    /// A handle that does nothing but extract — the "cache disabled" path.
    pub fn disabled() -> Self {
        Self {
            dir: None,
            file_name: String::new(),
            entries: HashMap::new(),
            seen_paths: HashSet::new(),
            corpus_walked: false,
            stats: CacheStats::default(),
            dirty: false,
        }
    }

    /// Zero the per-run counters. [`Self::begin_load`] calls this, and a Source
    /// may also call it directly, so the `--explain` line and the search TUI
    /// header describe this run's loads only — the counters otherwise accumulate
    /// across the refreshes that keep the handle (and its cache) alive.
    pub fn reset_stats(&mut self) {
        self.stats = CacheStats::default();
    }

    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    fn index_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(&self.file_name))
    }

    /// Parse the on-disk index. A torn or corrupted line is skipped — only the
    /// lines after it are lost, and those files simply re-extract, so the index
    /// heals itself on the next write.
    fn load_index(&mut self) {
        let Some(path) = self.index_path() else {
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(entry) = serde_json::from_str::<CachedFile>(line) {
                self.entries.insert(entry.fingerprint.path.clone(), entry);
            }
        }
    }

    /// Note that this load visited `path` — including on a cache hit, where
    /// `record` would never run — so pruning keeps it in the index.
    pub fn mark_seen(&mut self, path: &Path) {
        self.seen_paths.insert(path.to_path_buf());
    }

    /// Look up one file without extracting. `Some` only on an exact fingerprint
    /// hit, so a caller that must reuse *only part* of a file's work — and would
    /// otherwise have to guess from an empty result — can branch on the
    /// `Option` instead.
    pub fn lookup(&mut self, fingerprint: &FileFingerprint) -> Option<Vec<Message>> {
        let entry = self.entries.get(&fingerprint.path)?;
        if entry.fingerprint != *fingerprint {
            return None;
        }
        self.stats.files_seen += 1;
        self.stats.files_reused += 1;
        self.stats.messages_reused += entry.messages.len();
        Some(entry.messages.clone())
    }

    /// Record an extraction the caller performed after a [`Self::lookup`] miss,
    /// so it is written to the index and counted. The complement of `lookup`:
    /// together they let a caller split a file's work and cache half of it.
    pub fn record(&mut self, fingerprint: &FileFingerprint, messages: Vec<Message>) {
        self.stats.files_seen += 1;
        self.stats.files_extracted += 1;
        self.stats.messages_extracted += messages.len();
        self.seen_paths.insert(fingerprint.path.clone());
        self.entries.insert(
            fingerprint.path.clone(),
            CachedFile {
                fingerprint: fingerprint.clone(),
                messages,
            },
        );
        self.dirty = true;
    }

    /// Announce that this load is walking this Source's corpus, and restart
    /// the per-run counters.
    ///
    /// Every path the walk reaches is recorded through `reuse_or_extract`
    /// (hit or miss), so when `flush` runs, an index entry for a path that was
    /// never reached describes a file the corpus no longer contains. Without
    /// this call `seen_paths` is empty and carries no information at all, so
    /// `flush` keeps the index untouched — pruning runs only on an announced
    /// walk. The handle outlives the run (the TUI's `r` refresh reopens it), so
    /// the counters zero here would otherwise accumulate across loads.
    pub fn begin_load(&mut self) {
        self.reset_stats();
        self.seen_paths.clear();
        self.corpus_walked = true;
    }

    /// Decide the extraction path for one file.
    ///
    /// On a fingerprint hit the cached messages are returned and `extract` is
    /// never called. Otherwise `extract` runs and its result (including an
    /// error) is passed through verbatim — a miss is not an error.
    ///
    /// Both paths record the file as visited, so a `begin_load`-announced walk
    /// that reaches a file keeps its index entry even when the fingerprint
    /// already matched and nothing was extracted.
    pub fn reuse_or_extract<E>(
        &mut self,
        fingerprint: Option<&FileFingerprint>,
        extract: E,
    ) -> Result<Vec<Message>, SourceError>
    where
        E: FnOnce() -> Result<Vec<Message>, SourceError>,
    {
        self.stats.files_seen += 1;

        if let Some(fp) = fingerprint {
            self.seen_paths.insert(fp.path.clone());
            if let Some(entry) = self.entries.get(&fp.path) {
                if entry.fingerprint == *fp {
                    self.stats.files_reused += 1;
                    self.stats.messages_reused += entry.messages.len();
                    return Ok(entry.messages.clone());
                }
            }
        }

        let messages = extract()?;
        self.stats.files_extracted += 1;
        self.stats.messages_extracted += messages.len();
        if let Some(fp) = fingerprint {
            self.seen_paths.insert(fp.path.clone());
            self.entries.insert(
                fp.path.clone(),
                CachedFile {
                    fingerprint: fp.clone(),
                    messages: messages.clone(),
                },
            );
            self.dirty = true;
        }
        Ok(messages)
    }

    /// Rewrite the index atomically iff anything was extracted or stale
    /// entries were dropped.
    ///
    /// The write goes to a temporary file beside the index and is renamed into
    /// place, so a concurrent reader sees either the old index or the new one in
    /// full. A write failure is swallowed: results are identical without the
    /// cache, only speed differs, and a broken cache is not a user-facing error.
    pub fn flush(&mut self) {
        // Spec 0025: entries for paths this load never visited describe deleted
        // files and are rewritten out. Only a load that announced its walk
        // (`begin_load`) may trigger the prune — an empty `seen_paths` from a
        // load that walked nothing is "unknown", not "the corpus is empty". A
        // fully cached walk extracts nothing and leaves `dirty` unset, so the
        // prune marks the rewrite itself; otherwise a steady-state corpus would
        // never lose its deleted files' entries.
        if self.corpus_walked {
            let before = self.entries.len();
            self.entries
                .retain(|path, _| self.seen_paths.contains(path));
            if self.entries.len() != before {
                self.dirty = true;
            }
        }

        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(path) = self.index_path() else {
            return;
        };

        let mut buf = String::new();
        for entry in self.entries.values() {
            match serde_json::to_string(entry) {
                Ok(line) => {
                    buf.push_str(&line);
                    buf.push('\n');
                }
                // Unserialisable entries are simply not cached; the file will
                // re-extract next run. Never a hard failure.
                Err(_) => continue,
            }
        }

        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        if std::fs::write(&tmp, buf.as_bytes()).is_ok() {
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Keep one source name per file, so a name with a path separator cannot escape
/// the cache directory.
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Resolve the default cache directory: `$XDG_CACHE_HOME/llmhelper` (or the
/// platform equivalent).
pub fn default_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("llmhelper")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn msg(text: &str) -> Message {
        Message {
            source: "test".to_string(),
            session_id: "s1".to_string(),
            project: "/p".to_string(),
            model: None,
            role: "user".to_string(),
            timestamp: Some(DateTime::<Utc>::UNIX_EPOCH),
            text: text.to_string(),
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn header_line_is_terse_and_ungrouped() {
        let stats = CacheStats {
            sources: 4,
            files_seen: 20,
            files_reused: 19,
            files_extracted: 1,
            messages_reused: 15113,
            messages_extracted: 12,
        };
        // The TUI header shows the counts the user acts on, plain — grouping is
        // the `--explain` line's job.
        assert_eq!(stats.header_line(), "cache: 15113 reused, 12 re-read");
    }

    #[test]
    fn explain_line_names_the_sources_and_groups_big_numbers() {
        let stats = CacheStats {
            sources: 4,
            files_seen: 83,
            files_reused: 82,
            files_extracted: 1,
            messages_reused: 15113,
            messages_extracted: 12,
        };
        assert_eq!(
            stats.explain_line(),
            "cache: 4 sources, 82 files reused, 1 re-read (15,113 messages reused, 12 extracted)"
        );
    }

    #[test]
    fn explain_line_singularizes_one_source() {
        let stats = CacheStats {
            sources: 1,
            files_seen: 1,
            ..Default::default()
        };
        assert!(
            stats.explain_line().starts_with("cache: 1 source, "),
            "one contributing source must read `1 source`: {}",
            stats.explain_line()
        );
    }

    #[test]
    fn is_observed_only_when_a_file_was_seen() {
        assert!(!CacheStats::default().is_observed());
        assert!(CacheStats {
            files_seen: 1,
            ..Default::default()
        }
        .is_observed());
    }

    #[test]
    fn group_digits_separates_every_three_places() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(15113), "15,113");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn unchanged_fingerprint_reuses_without_extracting() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let mut cache = MessageCache::open(&dir.path().join("cache"), "test", true);

        let fp = FileFingerprint::capture(&src).unwrap();
        let first = cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("hello")]))
            .unwrap();
        assert_eq!(first.len(), 1);

        // Second pass: the closure must not run.
        let mut called = false;
        let second = cache
            .reuse_or_extract(Some(&fp), || {
                called = true;
                Ok(vec![msg("other")])
            })
            .unwrap();
        assert!(!called, "a fingerprint hit must not call extract");
        assert_eq!(second, first);
        let stats = cache.stats();
        assert_eq!(stats.files_reused, 1);
        assert_eq!(stats.files_extracted, 1);
        assert_eq!(stats.messages_reused, 1);
        assert_eq!(stats.messages_extracted, 1);
    }

    #[test]
    fn size_change_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let mut cache = MessageCache::open(&dir.path().join("cache"), "test", true);
        let fp1 = FileFingerprint::capture(&src).unwrap();
        cache
            .reuse_or_extract(Some(&fp1), || Ok(vec![msg("a")]))
            .unwrap();

        write(&src, "one\ntwo\n");
        let fp2 = FileFingerprint::capture(&src).unwrap();
        let mut called = false;
        let out = cache
            .reuse_or_extract(Some(&fp2), || {
                called = true;
                Ok(vec![msg("a"), msg("b")])
            })
            .unwrap();
        assert!(called, "a size change must re-extract");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn nanosecond_mtime_change_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "same-size\n");
        let fp1 = FileFingerprint::capture(&src).unwrap();

        // Rewrite with identical length, then force the mtime forward. std
        // cannot set mtimes, so we rewrite in a tight loop until the nanosecond
        // clock ticks — real filesystems have sub-ns granularity here.
        let mut fp2 = fp1.clone();
        for _ in 0..200 {
            write(&src, "SAME-SIZE\n");
            let candidate = FileFingerprint::capture(&src).unwrap();
            if candidate.mtime_ns != fp1.mtime_ns {
                fp2 = candidate;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let mut cache = MessageCache::open(&dir.path().join("cache"), "test", true);
        cache
            .reuse_or_extract(Some(&fp1), || Ok(vec![msg("old")]))
            .unwrap();
        let mut called = false;
        cache
            .reuse_or_extract(Some(&fp2), || {
                called = true;
                Ok(vec![msg("new")])
            })
            .unwrap();
        // Either the same-length rewrite was detected via mtime (called) or the
        // filesystem's mtime granularity hid it (not called) — assert the key
        // logic: a differing fingerprint is not reused.
        if fp2 != fp1 {
            assert!(called, "a changed fingerprint must re-extract");
        }
    }

    #[test]
    fn corrupted_index_line_self_heals() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        {
            let mut cache = MessageCache::open(&cache_dir, "test", true);
            cache
                .reuse_or_extract(Some(&fp), || Ok(vec![msg("a")]))
                .unwrap();
            cache.flush();
        }

        // Inject a corrupt line in the middle.
        let index = cache_dir.join("test.ndjson");
        let good = std::fs::read_to_string(&index).unwrap();
        std::fs::write(&index, format!("{good}{{ this is not json\n")).unwrap();

        let mut cache = MessageCache::open(&cache_dir, "test", true);
        let mut called = false;
        let out = cache
            .reuse_or_extract(Some(&fp), || {
                called = true;
                Ok(vec![msg("a")])
            })
            .unwrap();
        // The valid entry survives, so it is still a hit.
        assert!(!called);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn flush_then_reopen_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        {
            let mut cache = MessageCache::open(&cache_dir, "test", true);
            cache
                .reuse_or_extract(Some(&fp), || Ok(vec![msg("persisted")]))
                .unwrap();
            cache.flush();
        }
        let mut cache = MessageCache::open(&cache_dir, "test", true);
        let mut called = false;
        let out = cache
            .reuse_or_extract(Some(&fp), || {
                called = true;
                Ok(Vec::new())
            })
            .unwrap();
        assert!(!called, "a reopened cache must serve the persisted entry");
        assert_eq!(out[0].text, "persisted");
    }

    #[test]
    fn flush_is_a_noop_when_nothing_was_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        let mut cache = MessageCache::open(&cache_dir, "test", true);
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("a")]))
            .unwrap();
        cache.flush();
        let first_len = std::fs::metadata(cache_dir.join("test.ndjson"))
            .unwrap()
            .len();

        // Nothing extracted this run: the index must not be rewritten.
        let mut cache = MessageCache::open(&cache_dir, "test", true);
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("a")]))
            .unwrap();
        cache.flush();
        let second_len = std::fs::metadata(cache_dir.join("test.ndjson"))
            .unwrap()
            .len();
        assert_eq!(first_len, second_len);
    }

    #[test]
    fn top_level_ndjson_shape_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        let mut cache = MessageCache::open(&cache_dir, "test", true);
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("a")]))
            .unwrap();
        cache.flush();

        let content = std::fs::read_to_string(cache_dir.join("test.ndjson")).unwrap();
        let v: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert!(v.get("path").is_some());
        assert!(v.get("size").is_some());
        assert!(v.get("mtime_ns").is_some());
        assert!(v.get("messages").unwrap().is_array());
    }

    #[test]
    fn prune_drops_entries_for_paths_the_corpus_no_longer_contains() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone.jsonl");
        let kept = dir.path().join("kept.jsonl");
        write(&gone, "one\n");
        write(&kept, "two\n");
        let gone_fp = FileFingerprint::capture(&gone).unwrap();
        let kept_fp = FileFingerprint::capture(&kept).unwrap();

        // Warm both entries.
        let mut cache = MessageCache::open(dir.path(), "test", true);
        cache
            .reuse_or_extract(Some(&gone_fp), || Ok(vec![msg("gone")]))
            .unwrap();
        cache
            .reuse_or_extract(Some(&kept_fp), || Ok(vec![msg("kept")]))
            .unwrap();
        cache.flush();
        assert_eq!(cache.entries.len(), 2);

        // The file behind `gone` is deleted and its next load never visits it.
        std::fs::remove_file(&gone).unwrap();
        let mut cache = MessageCache::open(dir.path(), "test", true);
        assert_eq!(cache.entries.len(), 2, "stale entry loaded from disk");
        cache.begin_load();
        cache
            .reuse_or_extract(Some(&kept_fp), || Ok(vec![msg("kept")]))
            .unwrap();
        cache.flush();

        // The rewritten index describes only the corpus's own entry.
        let cache = MessageCache::open(dir.path(), "test", true);
        assert!(
            !cache.entries.contains_key(&gone_fp.path),
            "pruning must drop a path the corpus no longer contains"
        );
        assert!(
            cache.entries.contains_key(&kept_fp.path),
            "pruning must keep the visited entry"
        );
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn prune_never_runs_when_the_load_visited_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        let mut cache = MessageCache::open(dir.path(), "test", true);
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("a")]))
            .unwrap();
        cache.flush();

        // A later run that walks no files must not treat the whole index as
        // stale — it simply has nothing to rewrite.
        let mut cache = MessageCache::open(dir.path(), "test", true);
        assert_eq!(cache.entries.len(), 1);
        cache.flush();
        let cache = MessageCache::open(dir.path(), "test", true);
        assert!(
            cache.entries.contains_key(&fp.path),
            "an empty load must not prune the index it never walked"
        );
    }

    #[test]
    fn begin_load_resets_the_per_run_counters() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        let mut cache = MessageCache::disabled();
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("one")]))
            .unwrap();
        assert_eq!(cache.stats().files_seen, 1);

        // A second load (the TUI's `r` refresh) must report its own walk, not
        // the sum since process start.
        cache.begin_load();
        assert_eq!(cache.stats().files_seen, 0);
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("two")]))
            .unwrap();
        assert_eq!(cache.stats().files_seen, 1);
    }

    #[test]
    fn begin_load_resets_the_corpus_walk() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        // First load announces a walk that visits the file; its entry stays.
        let mut cache = MessageCache::open(dir.path(), "test", true);
        cache.begin_load();
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("one")]))
            .unwrap();
        cache.flush();
        assert_eq!(cache.entries.len(), 1);

        // A second load announces a fresh walk, and the still-present file is
        // revisited, so the entry survives both walks.
        cache.begin_load();
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("one again")]))
            .unwrap();
        cache.flush();
        assert!(
            cache.entries.contains_key(&fp.path),
            "a surviving file must survive its own Source's prune"
        );
    }

    #[test]
    fn reset_stats_zeroes_the_counters() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        write(&src, "one\n");
        let fp = FileFingerprint::capture(&src).unwrap();

        let mut cache = MessageCache::disabled();
        // A miss re-extracts, so the counters record one file and one message.
        cache
            .reuse_or_extract(Some(&fp), || Ok(vec![msg("hello")]))
            .unwrap();
        assert_eq!(cache.stats().files_seen, 1);
        assert_eq!(cache.stats().files_extracted, 1);

        cache.reset_stats();
        assert_eq!(cache.stats().files_seen, 0);
        assert_eq!(cache.stats().files_extracted, 0);
        assert_eq!(cache.stats().messages_extracted, 0);
    }

    #[test]
    fn stats_accumulate_until_an_explicit_reset() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jsonl");
        let b = dir.path().join("b.jsonl");
        write(&a, "one\n");
        write(&b, "two\n");
        let fp_a = FileFingerprint::capture(&a).unwrap();
        let fp_b = FileFingerprint::capture(&b).unwrap();

        let mut cache = MessageCache::disabled();
        cache
            .reuse_or_extract(Some(&fp_a), || Ok(vec![msg("one")]))
            .unwrap();
        cache
            .reuse_or_extract(Some(&fp_b), || Ok(vec![msg("two")]))
            .unwrap();
        assert_eq!(cache.stats().files_seen, 2, "the counters accumulate");
        assert_eq!(cache.stats().files_extracted, 2);
    }

    #[test]
    fn unwritable_directory_degrades_to_extract() {
        let dir = tempfile::tempdir().unwrap();
        // A file where a directory is expected: create_dir_all fails.
        let blocker = dir.path().join("blocker");
        write(&blocker, "not a dir");

        let mut cache = MessageCache::open(&blocker, "test", true);
        assert!(!cache.is_enabled());
        let out = cache
            .reuse_or_extract(None, || Ok(vec![msg("still works")]))
            .unwrap();
        assert_eq!(out.len(), 1);
        // Flush must not panic.
        cache.flush();
    }

    #[test]
    fn disabled_cache_extracts_every_time() {
        let mut cache = MessageCache::disabled();
        assert!(!cache.is_enabled());
        let mut calls = 0;
        for _ in 0..2 {
            cache
                .reuse_or_extract(None, || {
                    calls += 1;
                    Ok(vec![msg("a")])
                })
                .unwrap();
        }
        assert_eq!(calls, 2);
        assert_eq!(cache.stats().files_extracted, 2);
    }

    #[test]
    fn extract_errors_pass_through() {
        let mut cache = MessageCache::disabled();
        let err = cache
            .reuse_or_extract(None, || Err(SourceError::Unreadable("boom".to_string())))
            .unwrap_err();
        assert!(matches!(err, SourceError::Unreadable(_)));
    }

    #[test]
    fn concurrent_flushes_do_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let a = dir.path().join("a.jsonl");
        let b = dir.path().join("b.jsonl");
        write(&a, "one\n");
        write(&b, "two\n");

        let mut c1 = MessageCache::open(&cache_dir, "test", true);
        let mut c2 = MessageCache::open(&cache_dir, "test", true);
        let fpa = FileFingerprint::capture(&a).unwrap();
        let fpb = FileFingerprint::capture(&b).unwrap();
        c1.reuse_or_extract(Some(&fpa), || Ok(vec![msg("a")]))
            .unwrap();
        c2.reuse_or_extract(Some(&fpb), || Ok(vec![msg("b")]))
            .unwrap();
        c1.flush();
        c2.flush();

        // Reopening must parse whatever survived without panic.
        let mut c3 = MessageCache::open(&cache_dir, "test", true);
        let out = c3
            .reuse_or_extract(Some(&fpa), || Ok(vec![msg("again")]))
            .unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn stats_not_observed_when_nothing_seen() {
        let stats = CacheStats::default();
        assert!(!stats.is_observed());
    }
}

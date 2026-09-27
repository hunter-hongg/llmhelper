//! Integration tests for the message-extraction cache, driven through the real
//! CLI binary against the shared fixture tree.
//!
//! The cache memoizes `load_messages` only — the corpus `search` and
//! `export --messages` read. These tests pin the three properties a user
//! depends on:
//!
//! 1. **Transparency** — a cached run produces byte-identical output to an
//!    uncached run (the cache is an accelerator, never a filter).
//! 2. **Reuse across runs** — a second run reads its messages from the index
//!    that the first run wrote, and reports it.
//! 3. **Record reads are untouched** — `usage`/`export` (records) never write a
//!    cache index, because they never call `load_messages`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_llmhelper"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// The four source-path overrides for the shared fixture tree.
fn fixture_paths() -> Vec<String> {
    [
        "--claude-dir",
        fixture_dir().join("claude").to_str().unwrap(),
        "--opencode-db",
        fixture_dir()
            .join("opencode")
            .join("opencode.db")
            .to_str()
            .unwrap(),
        "--omp-dir",
        fixture_dir().join("omp").to_str().unwrap(),
        "--kilo-db",
        fixture_dir().join("kilo").join("kilo.db").to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// A private, empty directory for one test's cache index.
///
/// Isolated per test (and per config) so the tests never read the developer's
/// real `~/.cache/llmhelper`, and so a test's second run sees only what its
/// first run wrote. Returns the directory; the config file written beside it
/// points `cache.dir` at it.
fn temp_cache_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "llmhelper-cache-it-{}-{}-{}",
        std::process::id(),
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&dir).expect("create temp cache dir");
    dir
}

/// Write a config file that points the cache at `cache_dir` and returns its path.
fn config_pointing_at(dir: &Path, cache_dir: &Path) -> PathBuf {
    let cache = cache_dir.join("cache");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!("[cache]\ndir = {:?}\n", cache.to_string_lossy()),
    )
    .expect("write config");
    config
}

/// Run a read command with the fixture paths, a config file, and extra args.
fn run_with_config(subcommand: &str, config: &Path, extra_args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.arg("--config").arg(config);
    cmd.arg(subcommand);
    cmd.args(fixture_paths());
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run llmhelper {subcommand}: {e}"))
}

fn stdout_of(output: &Output, context: &str) -> String {
    assert!(
        output.status.success(),
        "{context} exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The cache index files written under a cache dir, sorted.
fn index_files(cache_dir: &Path) -> Vec<PathBuf> {
    let cache = cache_dir.join("cache");
    if !cache.exists() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&cache)
        .expect("read cache dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "ndjson").unwrap_or(false))
        .collect();
    files.sort();
    files
}

/// The fixture's own Claude transcript tree — the source the freshness test
/// appends to, on a copy.
///
/// Run a command against a *copy* of the fixture tree via
/// [`copy_fixture_tree`], so appending cannot disturb the read-only fixtures
/// every other test shares.
fn copy_fixture_tree(dest: &Path) -> Vec<String> {
    copy_dir(&fixture_dir(), dest);
    [
        "--claude-dir",
        dest.join("claude").to_str().unwrap(),
        "--opencode-db",
        dest.join("opencode").join("opencode.db").to_str().unwrap(),
        "--omp-dir",
        dest.join("omp").to_str().unwrap(),
        "--kilo-db",
        dest.join("kilo").join("kilo.db").to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|e| panic!("mkdir {}: {e}", to.display()));
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

/// A cached `search` is **byte-identical** to an uncached one.
///
/// This is the headline contract (spec 0025, story 20): every cache figure
/// lives behind `--explain`, so plain `--json` must not gain a field, not even
/// one that "only" differs. Comparing parsed JSON would forgive a field the
/// cache added; comparing raw stdout cannot.
#[test]
fn search_json_is_byte_identical_with_and_without_the_cache() {
    let dir = temp_cache_dir("search-identical");
    let config = config_pointing_at(&dir, &dir);

    let cold = run_with_config("search", &config, &["--json", "hello"]);
    let cached = run_with_config("search", &config, &["--json", "hello"]);
    let uncached = run_with_config("search", &config, &["--json", "--no-cache", "hello"]);

    let cached_out = stdout_of(&cached, "search (warm cache)");
    assert_eq!(stdout_of(&cold, "search (cold cache)"), cached_out);
    let uncached_out = stdout_of(&uncached, "search (--no-cache)");
    assert_eq!(
        cached_out, uncached_out,
        "a cached search must be byte-identical to an uncached one"
    );
    assert!(
        !cached_out.contains("\"cache\""),
        "normal --json must gain no cache keys: {cached_out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Spec 0026: the byte-identity contract holds in every match mode — the
/// corpus may rank differently, but cached and uncached runs of the same
/// query must never differ by a byte.
#[test]
fn search_regex_json_is_byte_identical_with_and_without_the_cache() {
    let dir = temp_cache_dir("search-regex-identical");
    let config = config_pointing_at(&dir, &dir);

    let args = &["--json", "--match", "regex", "cach(e|e)"];
    let cold = run_with_config("search", &config, args);
    let cached = run_with_config("search", &config, args);
    let uncached = run_with_config(
        "search",
        &config,
        &["--json", "--no-cache", "--match", "regex", "cach(e|e)"],
    );

    let cached_out = stdout_of(&cached, "search regex (warm cache)");
    assert_eq!(stdout_of(&cold, "search regex (cold cache)"), cached_out);
    assert_eq!(
        cached_out,
        stdout_of(&uncached, "search regex (--no-cache)"),
        "a cached regex search must be byte-identical to an uncached one"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_fuzzy_json_is_byte_identical_with_and_without_the_cache() {
    let dir = temp_cache_dir("search-fuzzy-identical");
    let config = config_pointing_at(&dir, &dir);

    let args = &["--json", "--match", "fuzzy", "cachwrit"];
    let cold = run_with_config("search", &config, args);
    let cached = run_with_config("search", &config, args);
    let uncached = run_with_config(
        "search",
        &config,
        &["--json", "--no-cache", "--match", "fuzzy", "cachwrit"],
    );

    let cached_out = stdout_of(&cached, "search fuzzy (warm cache)");
    assert_eq!(stdout_of(&cold, "search fuzzy (cold cache)"), cached_out);
    assert_eq!(
        cached_out,
        stdout_of(&uncached, "search fuzzy (--no-cache)"),
        "a cached fuzzy search must be byte-identical to an uncached one"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The same byte-identity guard for `export --messages`, the other
/// message-reading command.
#[test]
fn export_messages_is_byte_identical_with_and_without_the_cache() {
    let dir = temp_cache_dir("export-identical");
    let config = config_pointing_at(&dir, &dir);

    let cold = run_with_config("export", &config, &["--messages"]);
    let cached = run_with_config("export", &config, &["--messages"]);
    let uncached = run_with_config("export", &config, &["--messages", "--no-cache"]);

    let cached_out = stdout_of(&cached, "export --messages (warm cache)");
    assert_eq!(
        stdout_of(&cold, "export --messages (cold cache)"),
        cached_out
    );
    let uncached_out = stdout_of(&uncached, "export --messages (--no-cache)");
    assert_eq!(
        cached_out, uncached_out,
        "a cached export --messages must be byte-identical to an uncached one"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--explain` is where the cache becomes visible, in both formats: a `cache`
/// object in JSON, one line on stderr in table mode — and in JSON mode *only*
/// the payload, never both.
#[test]
fn explain_reports_the_cache_in_each_mode() {
    let dir = temp_cache_dir("explain-modes");
    let config = config_pointing_at(&dir, &dir);

    // Warm the cache first, so the reported figures are non-zero.
    run_with_config("search", &config, &["--text", "hello"]);

    let json = run_with_config("search", &config, &["--json", "--explain", "hello"]);
    let json_out = stdout_of(&json, "search --json --explain");
    let payload: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    assert!(
        payload["cache"]["files_seen"].as_u64().unwrap_or(0) > 0,
        "--explain JSON must carry a populated cache object: {json_out}"
    );
    assert!(
        payload["cache"]["sources"].as_u64().unwrap_or(0) > 0,
        "--explain JSON must count the contributing sources: {json_out}"
    );
    let json_stderr = String::from_utf8_lossy(&json.stderr);
    assert!(
        !json_stderr.contains("cache:"),
        "in JSON mode the payload says it once; stderr must not repeat it: {json_stderr}"
    );

    let table = run_with_config("search", &config, &["--text", "--explain", "hello"]);
    let table_stderr = stderr_of(&table);
    let line = table_stderr
        .lines()
        .find(|l| l.contains("cache:"))
        .unwrap_or_else(|| panic!("--explain must report the cache on stderr: {table_stderr}"));
    assert!(
        line.contains("sources") && line.contains("files reused") && line.contains("re-read"),
        "the --explain line must use the documented wording: {line}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A second run reuses the index the first run wrote, and `--explain` says so.
///
/// The first run is cold (extract, then write); the second is warm
/// (`files_reused` equals `files_seen`). The assertion is on the *reported*
/// reuse rather than the presence of the file, because the file existing is
/// not the property the user is shown.
#[test]
fn a_second_run_reuses_the_index_and_reports_it() {
    let dir = temp_cache_dir("reuse");
    let config = config_pointing_at(&dir, &dir);

    // Cold run: writes the index.
    let first = run_with_config("search", &config, &["--text", "--explain", "hello"]);
    let first_stderr = stderr_of(&first);
    assert!(
        first_stderr.contains("cache:"),
        "--explain must report cache statistics: {first_stderr}"
    );
    assert!(
        !index_files(&dir).is_empty(),
        "the cold run must write at least one index file"
    );

    // Warm run: reads them back. Every file must be reused, which is the
    // signal the `--explain` line now carries as `N files reused, 0 re-read`.
    let second = run_with_config("search", &config, &["--text", "--explain", "hello"]);
    let second_stderr = stderr_of(&second);
    let line = second_stderr
        .lines()
        .find(|l| l.contains("cache:"))
        .unwrap_or_else(|| panic!("warm run did not report cache stats: {second_stderr}"));
    let files = counts_before(line, "files reused")
        .unwrap_or_else(|| panic!("warm run did not report file reuse: {line}"));
    let re_read = counts_before(line, "re-read")
        .unwrap_or_else(|| panic!("warm run did not report re-reads: {line}"));
    assert_eq!(re_read, 0, "on a warm run nothing may be re-read: {line}");
    assert!(
        files > 0,
        "on a warm run files must actually be reused: {line}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Parse the number that precedes `follower` on an `--explain` cache line,
/// e.g. `82` from `cache: 4 sources, 82 files reused, 1 re-read (...)`.
fn counts_before(line: &str, follower: &str) -> Option<usize> {
    let before = line.split(follower).next()?;
    let token = before.split_whitespace().last()?;
    token.replace(',', "").parse().ok()
}

/// Freshness is mutation-tested: warm the cache, append a message to a
/// transcript, run again, and the new message must appear.
///
/// Appending changes the file size, so the fingerprint misses without any
/// mtime manipulation (std cannot set mtimes, and no dev-dependency is added
/// for this). This is the test behind story 2 — "the cache never shows me a
/// stale corpus" — which nothing else covers.
#[test]
fn appending_to_a_transcript_invalidates_its_cache_entry() {
    let root = temp_cache_dir("freshness-tree");
    let cache = temp_cache_dir("freshness-cache");
    let config = config_pointing_at(&cache, &cache);
    let sources = copy_fixture_tree(&root);

    let run = |args: &[&str]| {
        let mut cmd = Command::new(bin());
        cmd.arg("--config").arg(&config);
        cmd.arg("search");
        cmd.args(&sources);
        cmd.args(args);
        cmd.output().expect("run llmhelper")
    };

    // Warm the cache, then confirm the sentinel is absent. The query string is
    // echoed in the payload, so the check is on `hits`, not on the raw text.
    let before = run(&["--json", "zq-sentinel-marker"]);
    let before_out = stdout_of(&before, "warm search");
    let before_json: serde_json::Value = serde_json::from_str(&before_out).unwrap();
    assert!(
        before_json["hits"].as_array().unwrap().is_empty(),
        "the sentinel must not match before it is appended: {before_out}"
    );

    // Append one message to a real fixture transcript, changing its size.
    let transcript = root
        .join("claude")
        .join("-home-hunter-projects-test")
        .join("session-a.jsonl");
    let appended = concat!(
        r#"{"type":"user","message":{"role":"user","#,
        r#""content":[{"type":"text","text":"zq-sentinel-marker"}]},"#,
        r#""timestamp":"2026-09-13T12:00:00.000Z","sessionId":"session-a"}"#,
        "\n"
    );
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .expect("open transcript for append");
        f.write_all(appended.as_bytes()).expect("append message");
    }

    // The next run must see it: the file's size changed, so its entry missed.
    let after = run(&["--json", "zq-sentinel-marker"]);
    let after_out = stdout_of(&after, "search after append");
    let after_json: serde_json::Value = serde_json::from_str(&after_out).unwrap();
    assert!(
        !after_json["hits"].as_array().unwrap().is_empty(),
        "a message appended after the cache was warmed must appear: {after_out}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache);
}

/// Committed WAL changes must be visible even when the main DB is unchanged.
#[test]
fn sqlite_wal_commits_remain_fresh_in_search_and_message_export() {
    for source in ["opencode", "kilo"] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("messages.db");
        let wal = dir.path().join("messages.db-wal");
        let config = config_pointing_at(dir.path(), dir.path());
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"PRAGMA journal_mode=WAL;
               PRAGMA wal_autocheckpoint=0;
               CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, model TEXT);
               CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT);
               CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, time_created INTEGER, data TEXT);
               INSERT INTO session VALUES ('s', '/wal-test', 'model');
               INSERT INTO message VALUES ('m', 's', '{"role":"user"}');
               INSERT INTO part VALUES ('p', 'm', 1700000000000, '{"type":"text","text":"wal-before"}');"#,
        )
        .unwrap();
        drop(conn); // Checkpoint and remove WAL before warming an ordinary DB entry.
        assert!(!wal.exists());
        let missing = dir.path().join("absent");
        let run = |command: &str, args: &[&str]| {
            Command::new(bin())
                .arg("--config")
                .arg(&config)
                .arg(command)
                .arg("--claude-dir")
                .arg(&missing)
                .arg("--omp-dir")
                .arg(&missing)
                .arg("--opencode-db")
                .arg(if source == "opencode" { &db } else { &missing })
                .arg("--kilo-db")
                .arg(if source == "kilo" { &db } else { &missing })
                .args(args)
                .output()
                .unwrap()
        };
        let assert_fresh = |expected: &[&str]| {
            let cached = stdout_of(&run("search", &["--json", "wal-"]), "cached WAL search");
            let off = stdout_of(
                &run("search", &["--json", "--no-cache", "wal-"]),
                "uncached WAL search",
            );
            assert_eq!(cached, off, "{source}: WAL search must be fresh");
            let cached = stdout_of(&run("export", &["--messages"]), "cached WAL export");
            let off = stdout_of(
                &run("export", &["--messages", "--no-cache"]),
                "uncached WAL export",
            );
            assert_eq!(cached, off, "{source}: WAL export must be fresh");
            for text in expected {
                assert!(cached.contains(text), "missing {text}: {cached}");
            }
        };
        assert_fresh(&["wal-before"]);
        assert_fresh(&["wal-before"]); // Reuse a previously persisted DB entry.

        // Keep the writer alive across CLI processes so no close checkpoints it.
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        let before = std::fs::metadata(&db).unwrap();
        writer.execute_batch(
            r#"INSERT INTO part VALUES ('p2', 'm', 1700000000010, '{"type":"text","text":"wal-inserted"}');"#,
        ).unwrap();
        assert!(wal.exists());
        let after = std::fs::metadata(&db).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        assert_fresh(&["wal-before", "wal-inserted"]);
        writer
            .execute_batch(
                r#"UPDATE part SET data = '{"type":"text","text":"wal-updated"}' WHERE id = 'p';"#,
            )
            .unwrap();
        assert_eq!(
            before.modified().unwrap(),
            std::fs::metadata(&db).unwrap().modified().unwrap()
        );
        assert_fresh(&["wal-updated", "wal-inserted"]);

        drop(writer); // Removal checkpoints the new contents into the main DB.
        assert!(!wal.exists());
        assert_fresh(&["wal-updated", "wal-inserted"]);
        assert_fresh(&["wal-updated", "wal-inserted"]);
    }
}

/// A corrupted index line costs only a re-extraction: the file it described is
/// re-read and the new line is found. Nothing is surfaced as an error.
#[test]
fn a_corrupted_index_line_fails_open() {
    let root = temp_cache_dir("corrupt-tree");
    let cache = temp_cache_dir("corrupt-cache");
    let config = config_pointing_at(&cache, &cache);
    let sources = copy_fixture_tree(&root);

    let run = |args: &[&str]| {
        let mut cmd = Command::new(bin());
        cmd.arg("--config").arg(&config);
        cmd.arg("search");
        cmd.args(&sources);
        cmd.args(args);
        cmd.output().expect("run llmhelper")
    };

    let warm = run(&["--json", "hello"]);
    let warm_out = stdout_of(&warm, "warm search");

    // Corrupt the cached file entries themselves: replacing a line the parser
    // accepted with one it must reject exercises discard + re-extract + heal,
    // which appended junk after every valid line cannot.
    for index in index_files(&cache) {
        let body = std::fs::read_to_string(&index).expect("read index");
        let corrupted: Vec<String> = body
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.replace('{', ""))
            .collect();
        std::fs::write(&index, corrupted.join("\n") + "\n").expect("write corrupt index");
    }

    let after = run(&["--json", "hello"]);
    let after_out = stdout_of(&after, "search over a corrupted index");
    assert_eq!(
        warm_out, after_out,
        "a corrupt index must degrade to re-extraction, not to different results"
    );
    assert!(
        after.stderr.is_empty() || !String::from_utf8_lossy(&after.stderr).contains("error"),
        "a corrupt index must fail open silently: {}",
        String::from_utf8_lossy(&after.stderr)
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache);
}

/// An unwritable cache directory disables the cache silently: the search still
/// succeeds and produces the same output as a `--no-cache` run.
#[test]
fn an_unwritable_cache_dir_degrades_silently() {
    let dir = temp_cache_dir("unwritable");
    // Point the cache at a path *inside a regular file*, which can never be a
    // directory — the portable way to make creation fail for the whole tree.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("write blocker");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[cache]\ndir = {:?}\n",
            blocker.join("cache").to_string_lossy()
        ),
    )
    .expect("write config");

    let broken = run_with_config("search", &config, &["--json", "hello"]);
    let broken_out = stdout_of(&broken, "search with an unusable cache dir");
    let reference = run_with_config("search", &config, &["--json", "--no-cache", "hello"]);
    let reference_out = stdout_of(&reference, "search --no-cache");
    assert_eq!(
        broken_out, reference_out,
        "an unusable cache dir must degrade to today's behaviour, byte for byte"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--refresh-cache` discards the stored index and rebuilds it, so the run
/// reports a cold cache again.
#[test]
fn refresh_cache_forces_a_cold_rebuild() {
    let dir = temp_cache_dir("refresh");
    let config = config_pointing_at(&dir, &dir);

    // Warm the cache.
    run_with_config("search", &config, &["--text", "hello"]);
    // Then ask for a rebuild.
    let refreshed = run_with_config(
        "search",
        &config,
        &["--text", "--explain", "--refresh-cache", "hello"],
    );
    let stderr = stderr_of(&refreshed);
    let reused = counts_before(
        stderr
            .lines()
            .find(|l| l.contains("cache:"))
            .unwrap_or_else(|| panic!("--refresh-cache run did not report cache stats: {stderr}")),
        "files reused",
    )
    .unwrap_or_else(|| panic!("--refresh-cache run did not report reuse: {stderr}"));
    assert_eq!(reused, 0, "--refresh-cache must reuse nothing: {stderr}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--refresh-cache` forces a cold rebuild even when the stored index cannot
/// be deleted (a read-only cache directory): deletion is best-effort, and the
/// handle ignores its stored entries either way.
///
/// The regression this pins is serving entries from the very file
/// `--refresh-cache` was asked to replace — the one case where a stale entry
/// is not just stale but user-contradicted.
#[test]
fn refresh_cache_rebuilds_even_when_the_index_cannot_be_deleted() {
    let dir = temp_cache_dir("refresh-ro");
    let config = config_pointing_at(&dir, &dir);

    run_with_config("search", &config, &["--text", "hello"]);
    let index_dir = dir.join("cache");
    assert!(!index_files(&dir).is_empty(), "the warm run wrote no index");

    {
        use std::os::unix::fs::PermissionsExt;
        let mut ro = std::fs::metadata(&index_dir)
            .expect("index dir")
            .permissions();
        ro.set_mode(0o555);
        std::fs::set_permissions(&index_dir, ro).expect("chmod index dir");
    }

    let refreshed = run_with_config(
        "search",
        &config,
        &["--text", "--explain", "--refresh-cache", "hello"],
    );
    let stderr = stderr_of(&refreshed);
    let reused = counts_before(
        stderr
            .lines()
            .find(|l| l.contains("cache:"))
            .unwrap_or_else(|| panic!("--refresh-cache run did not report cache stats: {stderr}")),
        "files reused",
    )
    .unwrap_or_else(|| panic!("--refresh-cache run did not report reuse: {stderr}"));
    assert_eq!(
        reused, 0,
        "an undeletable index must still be ignored by --refresh-cache: {stderr}"
    );

    // Restore so the temp dir can be removed.
    {
        use std::os::unix::fs::PermissionsExt;
        let mut rw = std::fs::metadata(&index_dir).unwrap().permissions();
        rw.set_mode(0o755);
        std::fs::set_permissions(&index_dir, rw).expect("restore permissions");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Record-only reads never touch the cache: `usage` writes no index file.
///
/// The cache is a property of message extraction; a command that never calls
/// `load_messages` has nothing to memoize, and writing an empty index would
/// misrepresent the run as cached.
#[test]
fn record_reads_write_no_index() {
    let dir = temp_cache_dir("records");
    let config = config_pointing_at(&dir, &dir);

    let output = run_with_config("usage", &config, &["--json"]);
    assert!(
        output.status.success(),
        "usage failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        index_files(&dir).is_empty(),
        "a record-only read must not write a cache index: {:?}",
        index_files(&dir)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Deleting a source file and re-running rewrites the index without the
/// deleted file's entry (spec 0025's rewrite-out guarantee).
///
/// The cache is an index over a corpus, not an archive: an entry whose file is
/// gone can never be revalidated, and keeping it would mean a stale corpus
/// lurking behind every future search.
#[test]
fn deleting_a_source_file_drops_its_index_entry() {
    let tree = temp_cache_dir("delete-tree");
    let cache = temp_cache_dir("delete-cache");
    let config = config_pointing_at(&cache, &cache);
    let sources = copy_fixture_tree(&tree);

    let run = |args: &[&str]| {
        let mut cmd = Command::new(bin());
        cmd.arg("--config").arg(&config);
        cmd.arg("search");
        cmd.args(&sources);
        cmd.args(args);
        cmd.output().expect("run llmhelper")
    };

    // Warm the index over the intact tree.
    run(&["--text", "hello"]);
    let warm = count_entries(&cache);
    assert!(warm > 0, "the warm run wrote no index entries");

    // Delete one Claude transcript, then re-run against the shrunken tree.
    let claude = tree.join("claude");
    let mut victim: Option<PathBuf> = None;
    for project in std::fs::read_dir(&claude).expect("read claude dir") {
        let project = project.expect("project entry").path();
        if let Some(f) = std::fs::read_dir(&project)
            .expect("read project dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|f| f.extension().map(|x| x == "jsonl").unwrap_or(false))
        {
            std::fs::remove_file(&f).expect("delete transcript");
            victim = Some(f);
            break;
        }
    }
    let victim = victim.expect("the fixture has a Claude transcript to delete");

    let rerun = run(&["--text", "hello"]);
    assert!(
        rerun.status.success(),
        "post-delete run failed: {}",
        String::from_utf8_lossy(&rerun.stderr)
    );

    let body = index_files(&cache)
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let deleted = std::fs::canonicalize(&victim).unwrap_or_else(|_| victim.clone());
    assert!(
        !body.contains(&deleted.to_string_lossy().to_string()),
        "the deleted file's entry survived the rewrite: {body}"
    );
    let after = count_entries(&cache);
    assert!(
        after < warm,
        "the index must shrink after a file is deleted: {warm} -> {after}"
    );

    let _ = std::fs::remove_dir_all(&tree);
    let _ = std::fs::remove_dir_all(&cache);
}

/// An empty corpus prunes only its own Source's index.
///
/// Pruning keys on what *this* Source's load visited: a Source pointed at an
/// empty directory announces a walk over an empty corpus and rewrites its
/// index to nothing, while the other Sources' indexes keep every entry. A
/// naive "this run saw no files, prune everything" implementation would wipe
/// all four here.
#[test]
fn an_empty_corpus_prunes_only_its_own_index() {
    let dir = temp_cache_dir("isolate-prune");
    let config = config_pointing_at(&dir, &dir);

    run_with_config("search", &config, &["--text", "hello"]);
    let warm = count_entries(&dir);
    assert!(warm > 0, "the warm run wrote no index entries");

    // The Claude Source walks an empty directory; the other three keep their
    // shared-fixture paths, named explicitly so the run is unambiguous.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("create empty dir");
    let mut cmd = Command::new(bin());
    cmd.arg("--config").arg(&config);
    cmd.arg("search").arg("--text").arg("hello");
    cmd.arg("--claude-dir").arg(&empty);
    cmd.arg("--opencode-db")
        .arg(fixture_dir().join("opencode").join("opencode.db"));
    cmd.arg("--omp-dir").arg(fixture_dir().join("omp"));
    cmd.arg("--kilo-db")
        .arg(fixture_dir().join("kilo").join("kilo.db"));
    let output = cmd.output().expect("run llmhelper");
    assert!(
        output.status.success(),
        "isolated run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let claude_body = index_files(&dir)
        .iter()
        .map(|f| (f.clone(), std::fs::read_to_string(f).unwrap_or_default()))
        .find(|(f, _)| {
            f.file_name()
                .map(|n| n.to_string_lossy().starts_with("claude"))
                .unwrap_or(false)
        })
        .map(|(_, body)| body)
        .unwrap_or_default();
    let claude_lines = claude_body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        claude_lines, 0,
        "an empty corpus must prune its own Source's index: {claude_body}"
    );

    let after = count_entries(&dir);
    assert!(
        after > 0 && after < warm,
        "the other Sources' indexes must survive an unrelated Source's empty walk: \
        {warm} -> {after}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Total cached-file entries across every index under a cache dir.
fn count_entries(cache_dir: &Path) -> usize {
    index_files(cache_dir)
        .iter()
        .map(|f| {
            std::fs::read_to_string(f)
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
        })
        .sum()
}

/// A disabled cache (`cache.enabled = false`) writes nothing even when
/// `--messages` is read.
#[test]
fn config_can_disable_the_cache() {
    let dir = temp_cache_dir("disabled");
    let cache = dir.join("cache");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[cache]\nenabled = false\ndir = {:?}\n",
            cache.to_string_lossy()
        ),
    )
    .expect("write config");

    let output = run_with_config("export", &config, &["--messages"]);
    assert!(
        output.status.success(),
        "export --messages failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        index_files(&dir).is_empty(),
        "a disabled cache must not write an index: {:?}",
        index_files(&dir)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

fn stderr_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "command exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

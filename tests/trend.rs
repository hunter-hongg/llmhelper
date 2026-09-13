//! Integration tests for `trend`, driven through the real CLI binary against
//! the shared fixture tree. This is the CLI seam from spec 0022.
//!
//! The fixture records carry fixed absolute timestamps (2026-08-27/28), so the
//! assertions that need records present use a window wide enough to cover them
//! and verify the *structure* of the series — a count of buckets, oldest-first
//! ordering, zero-filled gaps — rather than binding to today's date. The
//! boundary math itself is unit-tested in `src/trend.rs` with explicit instants.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_llmhelper"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// The four source-path overrides for the shared fixture tree, which holds
/// seven records across four sources on 2026-08-27/28.
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

/// The four source-path overrides pointed at paths that do not exist, so a test
/// asserting a zero-loaded result can never observe the developer's real data.
fn absent_paths(base: &std::path::Path) -> Vec<String> {
    [
        "--claude-dir".to_string(),
        base.join("absent-claude").to_str().unwrap().to_string(),
        "--opencode-db".to_string(),
        base.join("absent-opencode.db")
            .to_str()
            .unwrap()
            .to_string(),
        "--omp-dir".to_string(),
        base.join("absent-omp").to_str().unwrap().to_string(),
        "--kilo-db".to_string(),
        base.join("absent-kilo.db").to_str().unwrap().to_string(),
    ]
    .to_vec()
}

fn run_with(paths: &[String], extra_args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.arg("trend");
    cmd.args(paths);
    cmd.args(extra_args);
    cmd.output().expect("failed to run llmhelper trend")
}

/// Run `trend` against the shared fixtures.
fn run(extra_args: &[&str]) -> Output {
    run_with(&fixture_paths(), extra_args)
}

fn json_of(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "trend exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("trend --json did not emit valid JSON")
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "trend exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn json_buckets_cover_the_window_and_are_dense() {
    let json = json_of(&run(&["--last", "30d", "--bucket", "1d", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    // 30 one-day buckets, one of which is the open "today so far" bucket.
    assert_eq!(buckets.len(), 30);
    // Every bucket carries all its fields, so a consumer can index by position.
    for b in buckets {
        assert!(b["since"].is_string());
        assert!(b["until"].is_string());
        assert!(b["sessions"].is_u64());
        assert!(b["messages"].is_u64());
        assert!(b["tokens"]["input"].is_u64());
    }
}

#[test]
fn json_buckets_are_oldest_first_and_contiguous() {
    let json = json_of(&run(&["--last", "10d", "--bucket", "1d", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 10);
    for w in buckets.windows(2) {
        let prev_until = w[0]["until"].as_str().unwrap();
        let next_since = w[1]["since"].as_str().unwrap();
        assert_eq!(
            prev_until, next_since,
            "buckets must be contiguous: {prev_until} != {next_since}"
        );
        assert!(w[0]["since"].as_str().unwrap() < w[1]["since"].as_str().unwrap());
    }
}

#[test]
fn json_last_bucket_is_the_open_one() {
    let json = json_of(&run(&["--last", "5d", "--bucket", "1d", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    let last = buckets.last().unwrap();
    // The final bucket runs to "now", so it is the only one whose width can be
    // a partial day. Its bounds are still well-formed.
    assert!(last["since"].as_str().unwrap() < last["until"].as_str().unwrap());
}

#[test]
fn a_multi_day_bucket_covers_the_whole_window() {
    // Regression guard for the bug this series was rewritten to fix: a
    // `--last 7d --bucket 1w` trend used to emit one bucket anchored on *today*,
    // covering a single day while claiming to be a week. The single bucket must
    // open seven local days before today.
    let json = json_of(&run(&["--last", "7d", "--bucket", "1w", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 1, "a 7d window in 1w buckets is one bucket");

    // The window opens on a local midnight, and its span is a full seven local
    // days — not the ~one day a today-anchored bucket would cover. Comparing the
    // two timestamps in UTC can shift by a DST hour, so allow a wide tolerance
    // while still rejecting a one-day bucket (which would differ by six days).
    let since = chrono::DateTime::parse_from_rfc3339(buckets[0]["since"].as_str().unwrap())
        .unwrap()
        .to_utc();
    let until = chrono::DateTime::parse_from_rfc3339(buckets[0]["until"].as_str().unwrap())
        .unwrap()
        .to_utc();
    let width = until - since;
    assert!(
        width >= chrono::Duration::days(6),
        "the bucket covers {width}, but a 7d/1w trend must span at least six days"
    );
}

#[test]
fn bucket_count_is_ceil_of_the_window_over_the_bucket() {
    // 30 days in 7-day buckets needs 5 rows to cover the span.
    let json = json_of(&run(&["--last", "30d", "--bucket", "1w", "--json"]));
    assert_eq!(json["buckets"].as_array().unwrap().len(), 5);

    // A bucket wider than the window yields exactly one bucket.
    let json = json_of(&run(&["--last", "3d", "--bucket", "1mo", "--json"]));
    assert_eq!(json["buckets"].as_array().unwrap().len(), 1);
}

#[test]
fn fixture_records_land_in_their_buckets_and_empty_buckets_stay_zero() {
    let json = json_of(&run(&["--last", "40d", "--bucket", "1d", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    let total: u64 = buckets
        .iter()
        .map(|b| b["sessions"].as_u64().unwrap())
        .sum();
    // All seven fixture records are inside the window.
    assert_eq!(total, 7, "expected the seven fixture sessions");

    // The two days that hold records are non-zero; the rest are explicit zeros.
    let non_zero = buckets
        .iter()
        .filter(|b| b["sessions"].as_u64().unwrap() > 0)
        .count();
    assert_eq!(non_zero, 2, "fixture records span exactly two buckets");
    let zeros = buckets
        .iter()
        .filter(|b| b["sessions"].as_u64().unwrap() == 0)
        .count();
    assert_eq!(zeros, buckets.len() - 2, "every quiet bucket is a zero row");
}

#[test]
fn a_source_scoped_cost_bucket_is_null_when_a_source_reports_none() {
    let json = json_of(&run(&["--last", "40d", "--bucket", "1d", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    // The bucket carrying the Claude fixture (which records no cost) must report
    // `null`, never a partial sum — ADR 0001.
    let with_claude = buckets
        .iter()
        .find(|b| b["cost"].is_null() && b["sessions"].as_u64().unwrap() > 0)
        .expect("a bucket holding a non-costing source should report null cost");
    assert!(with_claude["cost"].is_null());
}

#[test]
fn cost_is_never_summed_across_two_sources() {
    // The 2026-08-27 fixture bucket holds OpenCode *and* Kilo records, both of
    // which report a cost. A naive sum would print ~1.67; ADR 0001 scopes cost to
    // a single source, so the bucket must be unpriced.
    let json = json_of(&run(&["--last", "40d", "--bucket", "1d", "--json"]));
    let buckets = json["buckets"].as_array().unwrap();
    let multi = buckets
        .iter()
        .find(|b| b["sessions"].as_u64().unwrap() >= 4)
        .expect("the fixture tree has a multi-source day");
    assert!(
        multi["cost"].is_null(),
        "a bucket drawing on two sources must not carry a summed cost, got {}",
        multi["cost"]
    );

    // The contrast: narrowed to OpenCode alone, the same day *is* priced.
    let json = json_of(&run(&[
        "--last", "40d", "--bucket", "1d", "--source", "opencode", "--json",
    ]));
    let buckets = json["buckets"].as_array().unwrap();
    let priced = buckets
        .iter()
        .find(|b| b["sessions"].as_u64().unwrap() > 0)
        .expect("opencode fixture records exist");
    assert!(
        priced["cost"].is_f64(),
        "a single cost-reporting source must yield a numeric cost"
    );
}

#[test]
fn a_single_source_that_reports_no_cost_is_null() {
    // Claude records carry no cost, so even a bucket drawn entirely from Claude
    // is unpriced — the distinction between "no cost data" and "zero".
    let json = json_of(&run(&[
        "--last", "40d", "--bucket", "1d", "--source", "claude", "--json",
    ]));
    let buckets = json["buckets"].as_array().unwrap();
    let claude = buckets
        .iter()
        .find(|b| b["sessions"].as_u64().unwrap() > 0)
        .expect("claude fixture records exist");
    assert!(claude["cost"].is_null());
}

#[test]
fn csv_body_is_a_bare_header_plus_one_line_per_bucket() {
    let out = run(&["--last", "7d", "--bucket", "1d", "--csv"]);
    let body = stdout_of(&out);
    let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 8, "expected a header plus seven buckets");
    assert_eq!(
        lines[0],
        "bucket_since,bucket_until,sessions,messages,input,output,cache_read,cache_write,cost"
    );
    // Every row is parseable and has the same field count as the header.
    let fields = lines[0].split(',').count();
    for line in &lines[1..] {
        assert_eq!(line.split(',').count(), fields, "ragged CSV row: {line}");
    }
}

#[test]
fn csv_with_explain_keeps_the_funnel_on_stderr_and_the_body_parseable() {
    let out = run(&["--last", "7d", "--bucket", "1d", "--csv", "--explain"]);
    let body = stdout_of(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The funnel must not be interleaved into the CSV body.
    assert!(
        !body.contains("filters:"),
        "the funnel leaked into the CSV body: {body}"
    );
    assert!(stderr.contains("filters:"), "expected the funnel on stderr");
    // The body is still a bare header plus seven rows.
    assert_eq!(body.lines().filter(|l| !l.is_empty()).count(), 8);
}

#[test]
fn json_carries_diagnostics_only_when_empty_or_explained() {
    // Ordinary non-empty run: no diagnostics key at all.
    let json = json_of(&run(&["--last", "40d", "--bucket", "1d", "--json"]));
    assert!(
        json.get("diagnostics").is_none(),
        "a non-empty run must not gain a diagnostics key"
    );

    // --explain attaches it.
    let json = json_of(&run(&[
        "--last",
        "40d",
        "--bucket",
        "1d",
        "--json",
        "--explain",
    ]));
    assert!(json["diagnostics"].is_object());
    let matched = json["diagnostics"]["matched"].as_u64().unwrap();
    assert_eq!(matched, 7, "all seven fixture records matched");

    // An empty result attaches it even without --explain.
    let tmp = std::env::temp_dir().join("llmhelper-trend-empty");
    let empty = json_of(&run_with(
        &absent_paths(&tmp),
        &["--last", "3d", "--bucket", "1d", "--json"],
    ));
    assert!(empty["diagnostics"].is_object());
    assert_eq!(empty["diagnostics"]["loaded"].as_u64().unwrap(), 0);
    assert!(empty["diagnostics"]["blamed"].is_null());
}

#[test]
fn an_empty_trend_is_exit_zero_and_still_prints_every_bucket() {
    // The point of a trend: "a month of zero usage" is a valid, fully rendered
    // result, not an empty failure (spec 0020's contract).
    let tmp = std::env::temp_dir().join("llmhelper-trend-empty2");
    let out = run_with(
        &absent_paths(&tmp),
        &["--last", "5d", "--bucket", "1d", "--json"],
    );
    assert!(out.status.success(), "an empty trend must exit 0");
    let json = json_of(&out);
    let buckets = json["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 5, "every bucket is still emitted");
    for b in buckets {
        assert_eq!(b["sessions"].as_u64().unwrap(), 0);
        assert!(b["cost"].is_null());
    }
}

#[test]
fn empty_trend_table_still_prints_zero_rows() {
    let tmp = std::env::temp_dir().join("llmhelper-trend-empty3");
    let out = run_with(&absent_paths(&tmp), &["--last", "3d", "--bucket", "1d"]);
    let body = stdout_of(&out);
    // Header (sources + trend + blank + column header + dashes) then 3 rows.
    let rows: Vec<&str> = body.lines().filter(|l| l.starts_with("20")).collect();
    assert_eq!(rows.len(), 3, "expected three dated zero rows");
    for r in rows {
        assert!(r.contains('0'), "a zero row still carries its counts: {r}");
    }
}

#[test]
fn trend_has_no_group_by_flag() {
    // A bucket row is a bucket's totals; there is nowhere to put a per-dimension
    // breakdown, so `--group-by` is deliberately absent. Slicing a trend to one
    // project/model/source is what the filter flags already do.
    let out = run(&["--last", "7d", "--bucket", "1d", "--group-by", "project"]);
    assert!(
        !out.status.success(),
        "--group-by must not be accepted by trend"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("group-by"),
        "an unexpected flag must be rejected: {stderr}"
    );
}

#[test]
fn mutual_exclusion_and_missing_requirements_error() {
    // Both encodings at once is a user error.
    let out = run(&["--last", "7d", "--bucket", "1d", "--json", "--csv"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("mutually exclusive"));

    // --last is required.
    let out = run(&["--bucket", "1d"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--last is required"));

    // --bucket is required.
    let out = run(&["--last", "7d"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--bucket is required"));
}

#[test]
fn a_non_calendar_bucket_is_rejected_loudly() {
    // A duration has no local-day alignment, so it must fail rather than
    // silently produce a misaligned grid.
    let out = run(&["--last", "7d", "--bucket", "4h"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--bucket"),
        "error should name the flag: {stderr}"
    );
    assert!(
        stderr.contains("4h"),
        "error should echo the value: {stderr}"
    );
    assert!(
        stderr.contains("1d") && stderr.contains("1w") && stderr.contains("1mo"),
        "error should name the accepted keywords: {stderr}"
    );
}

#[test]
fn an_invalid_last_duration_is_rejected() {
    let out = run(&["--last", "7x", "--bucket", "1d"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--last"));
}

#[test]
fn filters_apply_before_bucketing() {
    // A project that matches nothing yields an all-zero, fully-rendered series
    // and a funnel that blames the project layer.
    let out = run(&[
        "--last",
        "40d",
        "--bucket",
        "1d",
        "--project",
        "/definitely-not-a-project",
        "--json",
    ]);
    let json = json_of(&out);
    assert_eq!(json["diagnostics"]["matched"].as_u64().unwrap(), 0);
    assert_eq!(json["diagnostics"]["blamed"].as_str().unwrap(), "project");
    let buckets = json["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 40);
    assert!(buckets.iter().all(|b| b["sessions"].as_u64().unwrap() == 0));
}

#[test]
fn explain_matched_equals_the_sum_of_the_bucket_rows() {
    // The invariant the whole design protects: the funnel's `matched` is the
    // records the rows are built from, so it must equal the rows' session sum.
    // A rolling `now - last` filter (as opposed to the bucket series' own
    // bounds) would silently drop up to a day of records between the window's
    // start and the first bucket, breaking exactly this equality.
    let json = json_of(&run(&[
        "--last",
        "40d",
        "--bucket",
        "1d",
        "--json",
        "--explain",
    ]));
    let matched = json["diagnostics"]["matched"].as_u64().unwrap();
    let summed: u64 = json["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["sessions"].as_u64().unwrap())
        .sum();
    assert_eq!(
        matched, summed,
        "diagnostics.matched ({matched}) must equal the sum of the bucket rows ({summed})"
    );
}

#[test]
fn a_sub_day_window_rounds_up_to_whole_buckets() {
    // A span of 25h needs two local days to cover it; flooring would silently
    // shorten the series to one row.
    let json = json_of(&run(&["--last", "25h", "--bucket", "1d", "--json"]));
    assert_eq!(json["buckets"].as_array().unwrap().len(), 2);

    // An exact day is one row, not two.
    let json = json_of(&run(&["--last", "24h", "--bucket", "1d", "--json"]));
    assert_eq!(json["buckets"].as_array().unwrap().len(), 1);
}

#[test]
fn source_filter_narrows_to_one_source() {
    let json = json_of(&run(&[
        "--last", "40d", "--bucket", "1d", "--source", "claude", "--json",
    ]));
    let total: u64 = json["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["sessions"].as_u64().unwrap())
        .sum();
    // The fixture tree holds two Claude sessions.
    assert_eq!(total, 2, "expected the two Claude fixture sessions");
}

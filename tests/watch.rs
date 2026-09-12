//! Integration tests for `watch`, driven through the real CLI binary against
//! the shared fixture tree. This is the CLI seam from spec 0019.
//!
//! The central assertion is an **equality against `usage`**: `watch --json` is
//! specified to be exactly the `usage --json` frame, so any divergence in the
//! loader, the window, the aggregation, the grouping or the serializer fails
//! here rather than being asserted twice by hand.

use chrono::{Local, TimeZone, Utc};
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

/// The four source-path overrides pointed at paths that do not exist, so a
/// test asserting an empty result can never observe the developer's real data.
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

fn run(subcommand: &str, paths: &[String], extra_args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.arg(subcommand).arg("--json");
    cmd.args(paths);
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run llmhelper {subcommand}: {e}"))
}

fn json_ok(subcommand: &str, paths: &[String], extra_args: &[&str]) -> serde_json::Value {
    let output = run(subcommand, paths, extra_args);
    assert!(
        output.status.success(),
        "llmhelper {subcommand} exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{subcommand} --json output was not valid JSON: {e}"))
}

/// `watch --json` must be byte-for-byte the `usage --json` frame for the same
/// flags. Asserting on bytes (not on parsed values) also pins field order.
fn assert_watch_matches_usage(paths: &[String], extra_args: &[&str]) {
    let watch = run("watch", paths, extra_args);
    let usage = run("usage", paths, extra_args);
    assert!(watch.status.success(), "watch failed: {:?}", watch);
    assert!(usage.status.success(), "usage failed: {:?}", usage);
    assert_eq!(
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&usage.stdout),
        "watch --json must be the usage --json frame for the same flags {:?}",
        extra_args
    );
}

#[test]
fn watch_json_is_byte_identical_to_usage_json() {
    assert_watch_matches_usage(&fixture_paths(), &[]);
}

#[test]
fn watch_json_matches_usage_for_every_grouping_dimension() {
    for dimension in ["source", "project", "model"] {
        assert_watch_matches_usage(&fixture_paths(), &["--group-by", dimension]);
    }
}

#[test]
fn watch_json_matches_usage_with_non_temporal_filters() {
    assert_watch_matches_usage(&fixture_paths(), &["--source", "opencode"]);
    assert_watch_matches_usage(&fixture_paths(), &["--project", "proj"]);
    assert_watch_matches_usage(&fixture_paths(), &["--model", "auto"]);
    assert_watch_matches_usage(&fixture_paths(), &["--last", "3650d"]);
}

#[test]
fn watch_json_matches_usage_for_a_calendar_window() {
    assert_watch_matches_usage(&fixture_paths(), &["--calendar", "--last", "1d"]);
}

#[test]
fn watch_json_frame_has_the_usage_shape() {
    let json = json_ok("watch", &fixture_paths(), &[]);
    assert!(json["groups"].is_array());
    assert!(json["sources"].is_array());
    assert_eq!(json["group_by"], "source");
    assert!(
        !json["groups"].as_array().unwrap().is_empty(),
        "fixtures should produce groups"
    );
}

#[test]
fn watch_json_reports_the_selected_grouping_dimension() {
    let json = json_ok("watch", &fixture_paths(), &["--group-by", "project"]);
    assert_eq!(json["group_by"], "project");
}

#[test]
fn watch_json_narrows_to_a_single_source() {
    let json = json_ok("watch", &fixture_paths(), &["--source", "opencode"]);
    let groups = json["groups"].as_array().unwrap();
    assert!(!groups.is_empty());
    for g in groups {
        assert_eq!(g["key"], "opencode", "unexpected group: {g}");
    }
}

/// A handful of records loaded into a temp tree with a Claude session dated
/// inside today's local day and another inside yesterday's.
fn write_calendar_fixture(base: &std::path::Path) {
    let proj = base.join("claude").join("-home-hunter-proj-watch");
    std::fs::create_dir_all(&proj).unwrap();
    for (file, ts, input, output) in [
        (
            "prev.jsonl",
            local_midnight_days_ago(1) + chrono::Duration::hours(9),
            100,
            50,
        ),
        (
            "curr.jsonl",
            local_midnight_days_ago(0) + chrono::Duration::minutes(1),
            200,
            100,
        ),
    ] {
        let obj = serde_json::json!({
            "type": "assistant",
            "timestamp": ts.format("%Y-%m-%dT%H:%M:%S.000Z").to_string(),
            "message": {
                "model": "auto",
                "usage": { "input_tokens": input, "output_tokens": output }
            }
        });
        std::fs::write(proj.join(file), obj.to_string()).unwrap();
    }
}

/// The instant of local midnight `days_ago` days before today, in UTC.
fn local_midnight_days_ago(days_ago: i64) -> chrono::DateTime<Utc> {
    let date = Local::now().date_naive() - chrono::Duration::days(days_ago);
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .with_timezone(&Utc)
}

fn total_sessions(json: &serde_json::Value) -> u64 {
    json["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["sessions"].as_u64().unwrap())
        .sum()
}

#[test]
fn watch_calendar_window_selects_only_today() {
    let dir = tempfile::tempdir().unwrap();
    write_calendar_fixture(dir.path());
    let mut paths = absent_paths(&dir.path().join("other"));
    // Point the Claude source at the temp tree we just wrote.
    let idx = paths.iter().position(|a| a == "--claude-dir").unwrap();
    paths[idx + 1] = dir.path().join("claude").to_str().unwrap().to_string();

    let today = json_ok("watch", &paths, &["--calendar", "--last", "1d"]);
    assert_eq!(
        total_sessions(&today),
        1,
        "only today's session belongs to the 1d calendar bucket"
    );

    let week = json_ok("watch", &paths, &["--calendar", "--last", "1w"]);
    assert_eq!(
        total_sessions(&week),
        2,
        "the trailing week includes today and yesterday"
    );
}

#[test]
fn watch_rejects_a_zero_interval_naming_the_flag_and_value() {
    let output = run("watch", &fixture_paths(), &["--interval", "0"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--interval"), "{stderr}");
    assert!(stderr.contains('0'), "{stderr}");
    assert!(stderr.contains("at least 1 second"), "{stderr}");
}

#[test]
fn watch_rejects_a_rolling_duration_in_calendar_mode() {
    let output = run("watch", &fixture_paths(), &["--calendar", "--last", "4h"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--last"), "{stderr}");
    assert!(stderr.contains("4h"), "{stderr}");
    // The same wording `usage` produces, because it is the same trait method.
    let usage = run("usage", &fixture_paths(), &["--calendar", "--last", "4h"]);
    assert_eq!(
        String::from_utf8_lossy(&usage.stderr),
        stderr,
        "watch and usage must reject the same window the same way"
    );
}

#[test]
fn watch_rejects_calendar_with_since() {
    let output = run(
        "watch",
        &fixture_paths(),
        &["--calendar", "--since", "2020-01-01T00:00:00Z"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--calendar"));
}

#[test]
fn watch_rejects_calendar_without_last() {
    let output = run("watch", &fixture_paths(), &["--calendar"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--calendar"));
}

#[test]
fn watch_rejects_since_and_last_together() {
    let output = run(
        "watch",
        &fixture_paths(),
        &["--since", "2020-01-01T00:00:00Z", "--last", "7d"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mutually exclusive"));
}

#[test]
fn watch_empty_window_exits_zero_with_an_empty_shape() {
    let dir = tempfile::tempdir().unwrap();
    let json = json_ok("watch", &absent_paths(dir.path()), &["--last", "1d"]);
    assert!(json["groups"].as_array().unwrap().is_empty());
    assert_eq!(json["group_by"], "source");
}

#[test]
fn watch_accepts_budget_flags_on_the_json_frame() {
    // Budgets are rendered by the TUI, but a bad budget flag is still a user
    // error and must be validated on the one-shot path too.
    let output = run(
        "watch",
        &fixture_paths(),
        &["--budget", "opencode:5.00", "--budget-window", "1d"],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bad = run(
        "watch",
        &fixture_paths(),
        &["--budget", "opencode:not-a-number"],
    );
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("--budget"));
}

#[test]
fn watch_accepts_a_custom_interval_on_the_json_path() {
    // `--interval` only governs the TUI loop, but it is still validated so an
    // invalid value cannot be silently accepted by a sampling invocation.
    let output = run("watch", &fixture_paths(), &["--interval", "60"]);
    assert!(output.status.success());
}

//! Integration tests driven via the real `usage --json` CLI binary.
//! This is the single testing seam from the spec.

use chrono::{Local, TimeZone, Utc};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Path to the release binary.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_llmhelper"))
}

/// Build the fixture directory path.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Run `llmhelper usage --json` with fixture paths and parse the output.
fn run_usage_json(extra_args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json"]);
    cmd.args([
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
    ]);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(
        output.status.success(),
        "llmhelper exited with {}: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(&stdout).unwrap()
}

fn run_sessions_json(extra_args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(bin());
    cmd.args(["sessions", "--json"]);
    cmd.args([
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
    ]);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(
        output.status.success(),
        "llmhelper sessions exited with {}: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(&stdout).unwrap()
}

#[test]
fn json_output_total_session_count() {
    let json = run_usage_json(&[]);
    let sources = json["sources"].as_array().unwrap();
    let total_records: usize = sources
        .iter()
        .map(|s| s["records"].as_u64().unwrap() as usize)
        .sum();
    // Claude: 2 sessions (session-a + session-b)
    // OpenCode: 2 sessions (ses_fix_001 + ses_fix_002)
    // OMP: 1 session (01fixomp)
    // Kilo: 2 sessions (ses_fix_kilo_001 + ses_fix_kilo_002)
    assert_eq!(total_records, 7);
}

#[test]
fn json_output_claude_cost_is_null() {
    let json = run_usage_json(&["--source", "claude"]);
    let groups = json["groups"].as_array().unwrap();
    for g in groups {
        assert_eq!(
            g["cost"],
            serde_json::Value::Null,
            "Claude cost must be null"
        );
    }
}

#[test]
fn json_output_opencode_cost_present() {
    let json = run_usage_json(&["--source", "opencode"]);
    let groups = json["groups"].as_array().unwrap();
    assert!(!groups.is_empty());
    let cost = groups[0]["cost"].as_f64();
    assert!(cost.is_some(), "OpenCode cost must be present");
}

#[test]
fn json_output_omp_cost_present() {
    let json = run_usage_json(&["--source", "omp"]);
    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let cost = groups[0]["cost"].as_f64();
    assert!(cost.is_some(), "OMP cost must be present");
    assert!((cost.unwrap() - 1.5).abs() < 1e-9);
}

#[test]
fn json_output_kilo_cost_present() {
    let json = run_usage_json(&["--source", "kilo"]);
    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let cost = groups[0]["cost"].as_f64();
    assert!(cost.is_some(), "Kilo cost must be present");
    assert!((cost.unwrap() - 1.5).abs() < 1e-9);
}

#[test]
fn json_output_kilo_source_filter() {
    let json = run_usage_json(&["--source", "kilo"]);
    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["key"], "kilo");
}

#[test]
fn json_output_token_sums_match_fixtures() {
    let json = run_usage_json(&[]);
    let groups = json["groups"].as_array().unwrap();
    let claude = groups.iter().find(|g| g["key"] == "claude").unwrap();
    assert_eq!(claude["tokens"]["input"], 650);
    // output includes reasoning (15) folded in.
    assert_eq!(claude["tokens"]["output"], 340);
    assert_eq!(claude["tokens"]["cache_read"], 30);
    assert_eq!(claude["tokens"]["cache_write"], 10);

    let oc = groups.iter().find(|g| g["key"] == "opencode").unwrap();
    assert_eq!(oc["tokens"]["input"], 59537);
    // OpenCode fixtures have reasoning=0, so output is unchanged.
    assert_eq!(oc["tokens"]["output"], 5017);

    let omp = groups.iter().find(|g| g["key"] == "omp").unwrap();
    assert_eq!(omp["tokens"]["input"], 3000);
    // output includes reasoning (1000) folded in.
    assert_eq!(omp["tokens"]["output"], 2500);
    assert_eq!(omp["tokens"]["cache_read"], 300);
    assert_eq!(omp["tokens"]["cache_write"], 75);
    assert_eq!(omp["sessions"], 1);
    assert_eq!(omp["messages"], 2);

    let kilo = groups.iter().find(|g| g["key"] == "kilo").unwrap();
    assert_eq!(kilo["tokens"]["input"], 4000);
    // output includes reasoning (1000) folded in.
    assert_eq!(kilo["tokens"]["output"], 3500);
    assert_eq!(kilo["tokens"]["cache_read"], 600);
    assert_eq!(kilo["tokens"]["cache_write"], 100);
    assert_eq!(kilo["sessions"], 2);
    assert_eq!(kilo["messages"], 5);
    // Kilo stores cost on the session row, so it must surface.
    assert!((kilo["cost"].as_f64().unwrap() - 1.5).abs() < 1e-9);
}

#[test]
fn json_output_source_filter() {
    let json = run_usage_json(&["--source", "claude"]);
    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["key"], "claude");
}

#[test]
fn json_output_project_filter() {
    let json = run_usage_json(&["--project", "/home/hunter"]);
    let groups = json["groups"].as_array().unwrap();
    // All four sources have projects under /home/hunter
    assert_eq!(groups.len(), 4);
}

#[test]
fn json_output_group_by_model() {
    let json = run_usage_json(&["--group-by", "model"]);
    let groups = json["groups"].as_array().unwrap();
    let auto = groups.iter().find(|g| g["key"] == "auto").unwrap();
    // claude session-b + omp + kilo (all record the routing alias "auto")
    assert_eq!(auto["sessions"], 3);
    let bp = groups.iter().find(|g| g["key"] == "big-pickle").unwrap();
    assert_eq!(bp["sessions"], 1);
    // The envelope's `id` is the grouping key, not the JSON blob.
    let kilo_auto = groups
        .iter()
        .find(|g| g["key"] == "kilo-auto/free")
        .unwrap();
    assert_eq!(kilo_auto["sessions"], 1);
    assert_eq!(kilo_auto["source"], "kilo");
}

#[test]
fn json_output_last_filter() {
    // --last 1d should include recent fixture sessions
    let json = run_usage_json(&["--last", "1d"]);
    let sources = json["sources"].as_array().unwrap();
    let total: usize = sources
        .iter()
        .map(|s| s["records"].as_u64().unwrap() as usize)
        .sum();
    // Fixture timestamps may be in the past; just verify it runs.
    let _ = total;

    // --last 100y should include everything
    let json = run_usage_json(&["--last", "36500d"]);
    let sources = json["sources"].as_array().unwrap();
    let total_all: usize = sources
        .iter()
        .map(|s| s["records"].as_u64().unwrap() as usize)
        .sum();
    assert_eq!(total_all, 7);
}

#[test]
fn json_output_group_by_flag_in_response() {
    let json = run_usage_json(&["--group-by", "model"]);
    assert_eq!(json["group_by"], "model");
}

#[test]
fn csv_output_header() {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--csv"]);
    cmd.args([
        "--claude-dir",
        fixture_dir().join("claude").to_str().unwrap(),
        "--opencode-db",
        fixture_dir()
            .join("opencode")
            .join("opencode.db")
            .to_str()
            .unwrap(),
    ]);
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let first_line = stdout.lines().next().unwrap();
    assert!(first_line.contains("group_key"));
    assert!(first_line.contains("sessions"));
    assert!(first_line.contains("input"));
    assert!(first_line.contains("cost"));
}

#[test]
fn cli_rejects_invalid_last_duration() {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--last", "7x"]);
    cmd.args([
        "--claude-dir",
        fixture_dir().join("claude").to_str().unwrap(),
        "--opencode-db",
        fixture_dir()
            .join("opencode")
            .join("opencode.db")
            .to_str()
            .unwrap(),
    ]);
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(!output.status.success(), "--last 7x should fail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid --last"));
}

#[test]
fn cli_rejects_json_and_csv_together() {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--csv"]);
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(!output.status.success());
}

#[test]
fn cli_rejects_since_and_last_together() {
    let mut cmd = Command::new(bin());
    cmd.args([
        "usage",
        "--json",
        "--since",
        "2025-01-01T00:00:00Z",
        "--last",
        "7d",
    ]);
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(!output.status.success());
}

#[test]
fn cli_rejects_unknown_source() {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--source", "invalid_source"]);
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(!output.status.success());
}

// --- diff integration tests ---

/// Create the four source dirs under `fixture_base` (even if empty) so
/// `discover_sources` does NOT fall back to the real user's stores, and point a
/// `Command` at them. Shared by the sliding and calendar diff helpers.
fn diff_cmd(fixture_base: &std::path::Path) -> Command {
    let claude_dir = fixture_base.join("claude");
    let omp_dir = fixture_base.join("omp");
    let opencode_dir = fixture_base.join("opencode");
    let kilo_dir = fixture_base.join("kilo");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::create_dir_all(&omp_dir).unwrap();
    fs::create_dir_all(&opencode_dir).unwrap();
    fs::create_dir_all(&kilo_dir).unwrap();
    fs::write(opencode_dir.join("opencode.db"), "").unwrap(); // empty sqlite stub
    fs::write(kilo_dir.join("kilo.db"), "").unwrap(); // empty sqlite stub

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--json"]);
    cmd.arg("--claude-dir").arg(claude_dir.to_str().unwrap());
    cmd.arg("--omp-dir").arg(omp_dir.to_str().unwrap());
    cmd.arg("--opencode-db")
        .arg(opencode_dir.join("opencode.db").to_str().unwrap());
    cmd.arg("--kilo-db")
        .arg(kilo_dir.join("kilo.db").to_str().unwrap());
    cmd
}

/// Helper: run `llmhelper diff --json` with ephemeral fixtures.
/// Timestamps are computed relative to `Utc::now()` at test-run time, so
/// they reliably fall into the prev / curr windows regardless of wall clock.
/// Windows use `--last {window}s --prev {window}s`.
fn run_diff_json(
    fixture_base: &std::path::Path,
    window: u64, // seconds for --last / --prev
    extra_args: &[&str],
) -> serde_json::Value {
    let mut cmd = diff_cmd(fixture_base);
    let dur = format!("{}s", window);
    cmd.arg("--last").arg(&dur);
    cmd.arg("--prev").arg(&dur);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(
        output.status.success(),
        "llmhelper diff exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(&stdout).unwrap()
}

/// Helper: run `llmhelper diff --json --calendar` with the given keywords.
fn run_diff_json_calendar(
    fixture_base: &std::path::Path,
    last: &str,
    prev: &str,
    extra_args: &[&str],
) -> serde_json::Value {
    let mut cmd = diff_cmd(fixture_base);
    cmd.arg("--calendar");
    cmd.arg("--last").arg(last);
    cmd.arg("--prev").arg(prev);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper");
    assert!(
        output.status.success(),
        "llmhelper diff --calendar exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(&stdout).unwrap()
}

// Window durations used by all diff tests.
// --last W --prev W gives:
//   prev window = [now - 2W, now - W]   (half-open via filter's > until)
//   curr window = [now - W,  now]
const DIFF_WINDOW_SECS: u64 = 86400; // 1 day

// Fixture timestamps are placed in the middle of their respective windows so
// they never straddle the boundary regardless of when the test runs.
fn diff_prev_ts() -> String {
    // Middle of prev window: now - 1.5 * W
    let offset = DIFF_WINDOW_SECS * 3 / 2 + 60; // extra minute of safety margin
    (Utc::now() - chrono::Duration::seconds(offset as i64))
        .format("%Y-%m-%dT%H:%M:%S.000Z")
        .to_string()
}
fn diff_curr_ts() -> String {
    // Middle of curr window: now - W / 2
    let offset = DIFF_WINDOW_SECS / 2 + 60;
    (Utc::now() - chrono::Duration::seconds(offset as i64))
        .format("%Y-%m-%dT%H:%M:%S.000Z")
        .to_string()
}

fn make_session(
    base: &std::path::Path,
    project_dir: &str,
    file: &str,
    ts: &str,
    input: u64,
    output: u64,
) {
    let proj_dir = base.join("claude").join(project_dir);
    fs::create_dir_all(&proj_dir).unwrap();
    let obj = serde_json::json!({
        "type": "assistant",
        "timestamp": ts,
        "message": {
            "model": "auto",
            "usage": {
                "input_tokens": input,
                "output_tokens": output
            }
        }
    });
    fs::write(proj_dir.join(file), obj.to_string()).unwrap();
}

/// A pair of Claude sessions in the same project with a token growth.
fn build_growth_fixture(base: &std::path::Path) {
    let prev_ts = diff_prev_ts();
    let curr_ts = diff_curr_ts();
    make_session(
        base,
        "-home-hunter-proj-growth",
        "session-prev.jsonl",
        &prev_ts,
        100,
        50,
    );
    make_session(
        base,
        "-home-hunter-proj-growth",
        "session-curr.jsonl",
        &curr_ts,
        200,
        100,
    );
}

/// A pair of Claude sessions with a token shrink.
fn build_shrink_fixture(base: &std::path::Path) {
    let prev_ts = diff_prev_ts();
    let curr_ts = diff_curr_ts();
    make_session(
        base,
        "-home-hunter-proj-shrink",
        "session-prev.jsonl",
        &prev_ts,
        200,
        100,
    );
    make_session(
        base,
        "-home-hunter-proj-shrink",
        "session-curr.jsonl",
        &curr_ts,
        100,
        50,
    );
}

/// One Claude session in the current window only.
fn build_new_fixture(base: &std::path::Path) {
    let curr_ts = diff_curr_ts();
    make_session(
        base,
        "-home-hunter-proj-new",
        "session-curr.jsonl",
        &curr_ts,
        50,
        25,
    );
}

/// One Claude session in the previous window only.
fn build_removed_fixture(base: &std::path::Path) {
    let prev_ts = diff_prev_ts();
    make_session(
        base,
        "-home-hunter-proj-removed",
        "session-prev.jsonl",
        &prev_ts,
        50,
        25,
    );
}

#[test]
fn diff_json_positive_delta() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "project"]);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["presence"], "both");
    assert_eq!(row["delta"]["tokens"]["input"], 100);
    assert!(row["delta"]["pct"]["input"].is_f64());
}

#[test]
fn diff_json_negative_delta() {
    let dir = tempfile::tempdir().unwrap();
    build_shrink_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "project"]);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["presence"], "both");
    assert_eq!(row["delta"]["tokens"]["input"], -100);
}

#[test]
fn diff_json_new_group() {
    let dir = tempfile::tempdir().unwrap();
    build_new_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "project"]);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["presence"], "new");
    assert!(rows[0]["prev"].is_null());
}

#[test]
fn diff_json_removed_group() {
    let dir = tempfile::tempdir().unwrap();
    build_removed_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "project"]);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["presence"], "removed");
    assert!(rows[0]["curr"].is_null());
}

#[test]
fn diff_json_source_filter() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "source"]);
    let rows = json["rows"].as_array().unwrap();
    // Only claude is present in this fixture
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"], "claude");
}

#[test]
fn diff_json_group_by_model() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "model"]);
    assert_eq!(json["group_by"], "model");
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"], "auto");
}

#[test]
fn diff_json_pct_none_when_prev_zero() {
    let dir = tempfile::tempdir().unwrap();
    build_new_fixture(dir.path()); // prev is empty → zero

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &["--group-by", "project"]);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows[0]["presence"], "new");
    assert!(
        rows[0]["delta"]["pct"]["input"].is_null(),
        "pct must be null when prev is zero"
    );
}

#[test]
fn diff_json_cost_null_for_claude() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &[]);
    let rows = json["rows"].as_array().unwrap();
    // Claude has no cost; delta.cost must be null
    for row in rows {
        assert!(
            row["delta"]["cost"].is_null(),
            "Claude cost delta must be null"
        );
    }
}

#[test]
fn diff_json_empty_curr_window() {
    // Only a prev-window session exists → curr is empty
    let dir = tempfile::tempdir().unwrap();
    build_removed_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &[]);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["presence"], "removed");
}

#[test]
fn diff_csv_output_header() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    fs::create_dir_all(dir.path().join("omp")).unwrap();
    let opencode_dir = dir.path().join("opencode");
    fs::create_dir_all(&opencode_dir).unwrap();
    fs::write(opencode_dir.join("opencode.db"), "").unwrap();

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--csv"]);
    cmd.arg("--last").arg(format!("{}s", DIFF_WINDOW_SECS));
    cmd.arg("--prev").arg(format!("{}s", DIFF_WINDOW_SECS));
    cmd.arg("--claude-dir")
        .arg(dir.path().join("claude").to_str().unwrap());
    cmd.arg("--omp-dir")
        .arg(dir.path().join("omp").to_str().unwrap());
    cmd.arg("--opencode-db")
        .arg(opencode_dir.join("opencode.db").to_str().unwrap());
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(
        output.status.success(),
        "diff --csv failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let header = stdout.lines().next().unwrap();
    assert!(header.contains("group_key"));
    assert!(header.contains("presence"));
    assert!(header.contains("delta_input"));
    assert!(header.contains("delta_output"));
    assert!(header.contains("delta_sessions"));
}

#[test]
fn diff_cli_rejects_invalid_last() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--last", "7x", "--prev", "7d"]);
    cmd.arg("--claude-dir")
        .arg(dir.path().join("claude").to_str().unwrap());
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid --last"));
}

#[test]
fn diff_cli_rejects_invalid_prev() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--last", "7d", "--prev", "7x"]);
    cmd.arg("--claude-dir")
        .arg(dir.path().join("claude").to_str().unwrap());
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid --prev"));
}

#[test]
fn diff_cli_rejects_missing_prev() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--last", "7d"]);
    cmd.arg("--claude-dir")
        .arg(dir.path().join("claude").to_str().unwrap());
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--prev"));
}

#[test]
fn diff_cli_rejects_missing_last() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--prev", "7d"]);
    cmd.arg("--claude-dir")
        .arg(dir.path().join("claude").to_str().unwrap());
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--last"));
}

#[test]
fn diff_cli_rejects_json_and_csv() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--json", "--csv", "--last", "7d", "--prev", "7d"]);
    cmd.arg("--claude-dir")
        .arg(dir.path().join("claude").to_str().unwrap());
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success());
}

#[test]
fn diff_json_windows_metadata() {
    let dir = tempfile::tempdir().unwrap();
    build_growth_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &[]);
    assert!(json["windows"]["previous"]["since"].is_string());
    assert!(json["windows"]["previous"]["until"].is_string());
    assert!(json["windows"]["current"]["since"].is_string());
    assert!(json["windows"]["current"]["until"].is_string());
}

#[test]
fn diff_json_all_windows_empty() {
    // Fixture with no sessions in either window
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("claude")).unwrap();

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &[]);
    let rows = json["rows"].as_array().unwrap();
    assert!(rows.is_empty());
}

// --- calendar-aligned diff tests ---

/// Format an instant the way session fixtures expect (`%Y-%m-%dT%H:%M:%S.000Z`).
fn iso(ts: chrono::DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%M:%S.000Z").to_string()
}

/// The instant of local midnight `days_ago` days before today, in UTC.
fn local_midnight_days_ago(days_ago: i64) -> chrono::DateTime<Utc> {
    let today = Local::now().date_naive();
    let date = today - chrono::Duration::days(days_ago);
    let naive = date.and_hms_opt(0, 0, 0).unwrap();
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap()
        .with_timezone(&Utc)
}

/// Two Claude sessions: one inside yesterday's local day, one inside today's.
fn build_calendar_fixture(base: &std::path::Path) {
    // A few hours after yesterday's local midnight and a few hours after
    // today's — both safely inside their respective `1d` buckets.
    let prev_ts = local_midnight_days_ago(1) + chrono::Duration::hours(9);
    let curr_ts = local_midnight_days_ago(0) + chrono::Duration::minutes(1);
    make_session(
        base,
        "-home-hunter-proj-cal",
        "session-prev.jsonl",
        &iso(prev_ts),
        100,
        50,
    );
    make_session(
        base,
        "-home-hunter-proj-cal",
        "session-curr.jsonl",
        &iso(curr_ts),
        200,
        100,
    );
}

#[test]
fn diff_calendar_windows_are_adjacent_and_midnight_anchored() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let json = run_diff_json_calendar(dir.path(), "1d", "1d", &[]);
    let prev_until = json["windows"]["previous"]["until"].as_str().unwrap();
    let curr_since = json["windows"]["current"]["since"].as_str().unwrap();
    let prev_since = json["windows"]["previous"]["since"].as_str().unwrap();

    // Adjacent and non-overlapping: prev ends exactly where current begins.
    assert_eq!(prev_until, curr_since);
    // Anchored to local midnight today (current) and yesterday (previous).
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(curr_since).unwrap(),
        local_midnight_days_ago(0)
    );
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(prev_since).unwrap(),
        local_midnight_days_ago(1)
    );
    // The yesterday session falls in the previous bucket, today's in current.
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["presence"], "both");
    assert_eq!(rows[0]["delta"]["tokens"]["input"], 100);
}

#[test]
fn diff_calendar_unequal_buckets_widen_the_previous_window() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let json = run_diff_json_calendar(dir.path(), "1d", "1w", &[]);
    let prev_since = json["windows"]["previous"]["since"].as_str().unwrap();
    // A `1w` previous bucket is the 7 local days before today's midnight.
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(prev_since).unwrap(),
        local_midnight_days_ago(7)
    );
}

#[test]
fn diff_calendar_group_by_project_still_groups() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let json = run_diff_json_calendar(dir.path(), "1d", "1d", &["--group-by", "project"]);
    assert_eq!(json["group_by"], "project");
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["key"].as_str().unwrap().contains("cal"));
}

#[test]
fn diff_without_calendar_stays_sliding_not_midnight_anchored() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let json = run_diff_json(dir.path(), DIFF_WINDOW_SECS, &[]);
    let curr_since = json["windows"]["current"]["since"].as_str().unwrap();
    let curr_since = chrono::DateTime::parse_from_rfc3339(curr_since).unwrap();
    // A rolling `1d` window ends "now minus a day", which is almost never local
    // midnight — proving the calendar anchoring did not leak in.
    assert_ne!(curr_since, local_midnight_days_ago(0));
    assert_ne!(curr_since, local_midnight_days_ago(1));
}

#[test]
fn diff_calendar_rejects_a_rolling_duration_for_last() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let mut cmd = diff_cmd(dir.path());
    cmd.args(["--calendar", "--last", "4h", "--prev", "1d"]);
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success(), "--calendar --last 4h should fail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--last"), "stderr was: {stderr}");
    assert!(stderr.contains("4h"), "stderr was: {stderr}");
}

#[test]
fn diff_calendar_rejects_a_rolling_duration_for_prev() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let mut cmd = diff_cmd(dir.path());
    cmd.args(["--calendar", "--last", "1d", "--prev", "2d"]);
    let output = cmd.output().expect("failed to run llmhelper diff");
    assert!(!output.status.success(), "--calendar --prev 2d should fail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--prev"), "stderr was: {stderr}");
    assert!(stderr.contains("2d"), "stderr was: {stderr}");
}

// --- calendar-aligned usage / report tests ---

/// Run `llmhelper usage --json` against a fixture base that contains only a
/// `claude` directory (built by [`build_calendar_fixture`]). Every other source
/// is pointed at a nonexistent path under the same temp base, so no real user
/// data leaks into the assertion.
fn run_usage_calendar_json(base: &std::path::Path, extra_args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json"]);
    cmd.arg("--claude-dir").arg(base.join("claude"));
    cmd.arg("--opencode-db").arg(base.join("absent.db"));
    cmd.arg("--omp-dir").arg(base.join("absent-omp"));
    cmd.arg("--kilo-db").arg(base.join("absent-kilo.db"));
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper usage");
    assert!(
        output.status.success(),
        "llmhelper usage exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap()
}

/// Total sessions across the *filtered* groups of a `usage --json` payload.
///
/// Note the `sources[].records` field counts records *loaded* per source, not
/// records surviving the filter, so it cannot be used to observe a window.
fn usage_total_sessions(json: &serde_json::Value) -> u64 {
    json["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["sessions"].as_u64().unwrap())
        .sum()
}

#[test]
fn usage_calendar_window_selects_only_today() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    // `--calendar --last 1d` = today so far → only today's session.
    let json = run_usage_calendar_json(dir.path(), &["--calendar", "--last", "1d"]);
    assert_eq!(usage_total_sessions(&json), 1);
    let groups = json["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["tokens"]["input"], 200);
    assert_eq!(groups[0]["tokens"]["output"], 100);
}

#[test]
fn usage_calendar_week_includes_yesterday_but_not_before() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let json = run_usage_calendar_json(dir.path(), &["--calendar", "--last", "1w"]);
    // The trailing 7 local days include today and yesterday.
    assert_eq!(usage_total_sessions(&json), 2);
}

#[test]
fn usage_without_calendar_includes_yesterday_in_a_rolling_day() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    // A rolling `2d` window always reaches back past yesterday's local midnight,
    // so it includes the previous session — whereas the `1d` calendar window
    // above did not. This shows the two derivations genuinely differ rather than
    // the calendar path merely happening to match a rolling one.
    let json = run_usage_calendar_json(dir.path(), &["--last", "2d"]);
    assert_eq!(usage_total_sessions(&json), 2);
}

#[test]
fn usage_calendar_rejects_a_rolling_duration() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--claude-dir"])
        .arg(dir.path().join("absent-claude"))
        .args(["--opencode-db"])
        .arg(dir.path().join("absent.db"))
        .args(["--omp-dir"])
        .arg(dir.path().join("absent-omp"))
        .args(["--kilo-db"])
        .arg(dir.path().join("absent-kilo.db"))
        .args(["--calendar", "--last", "4h"]);
    let output = cmd.output().expect("failed to run llmhelper usage");
    assert!(!output.status.success(), "--calendar --last 4h should fail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--last"), "stderr was: {stderr}");
    assert!(stderr.contains("4h"), "stderr was: {stderr}");
}

#[test]
fn usage_calendar_rejects_since() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--claude-dir"])
        .arg(dir.path().join("absent-claude"))
        .args(["--opencode-db"])
        .arg(dir.path().join("absent.db"))
        .args(["--omp-dir"])
        .arg(dir.path().join("absent-omp"))
        .args(["--kilo-db"])
        .arg(dir.path().join("absent-kilo.db"))
        .args(["--calendar", "--since", "2026-01-01T00:00:00Z"]);
    let output = cmd.output().expect("failed to run llmhelper usage");
    assert!(!output.status.success(), "--calendar --since should fail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("mutually exclusive"),
        "stderr was: {stderr}"
    );
}

#[test]
fn usage_calendar_requires_last() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());

    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--claude-dir"])
        .arg(dir.path().join("absent-claude"))
        .args(["--opencode-db"])
        .arg(dir.path().join("absent.db"))
        .args(["--omp-dir"])
        .arg(dir.path().join("absent-omp"))
        .args(["--kilo-db"])
        .arg(dir.path().join("absent-kilo.db"))
        .arg("--calendar");
    let output = cmd.output().expect("failed to run llmhelper usage");
    assert!(!output.status.success(), "--calendar alone should fail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("requires --last"), "stderr was: {stderr}");
}

#[test]
fn report_calendar_header_names_the_local_midnight_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let report_path = tmp.path().join("cal.md");
    let mut cmd = Command::new(bin());
    cmd.args(["report", "--output"])
        .arg(&report_path)
        .args(["--calendar", "--last", "1d"]);
    for flag in report_source_flags() {
        cmd.arg(flag);
    }
    let output = cmd.output().expect("failed to run llmhelper report");
    assert!(
        output.status.success(),
        "report exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let md = std::fs::read_to_string(&report_path).unwrap();
    assert!(
        md.contains("last 1d (calendar, local midnight)"),
        "report header missing calendar marker:\n{md}"
    );
}

#[test]
fn usage_calendar_matching_budget_is_not_reported_as_clipped() {
    let dir = tempfile::tempdir().unwrap();
    build_calendar_fixture(dir.path());
    // A `1d` budget and `--calendar --last 1d` describe the exact same window,
    // so the report must not claim it only measured a clipped range.
    let tmp = tempfile::tempdir().unwrap();
    let report_path = tmp.path().join("budget.md");
    let mut cmd = Command::new(bin());
    cmd.args(["report", "--output"])
        .arg(&report_path)
        .args(["--calendar", "--last", "1d"])
        .args(["--budget", "claude:5.00", "--budget-window", "1d"])
        .arg("--claude-dir")
        .arg(dir.path().join("claude"))
        .arg("--opencode-db")
        .arg(dir.path().join("nope.db"))
        .arg("--omp-dir")
        .arg(dir.path().join("nope-omp"))
        .arg("--kilo-db")
        .arg(dir.path().join("nope-kilo.db"));
    let output = cmd.output().expect("failed to run llmhelper report");
    assert!(
        output.status.success(),
        "report exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let md = std::fs::read_to_string(&report_path).unwrap();
    assert!(md.contains("## Budget"), "budget section missing:\n{md}");
    assert!(
        !md.contains("only records from"),
        "a 1d budget under --calendar --last 1d must not be flagged as clipped:\n{md}"
    );
}

#[test]
fn sessions_json_total_count() {
    let arr = run_sessions_json(&[]);
    let sessions = arr.as_array().unwrap();
    // 7 sessions across fixtures
    assert_eq!(sessions.len(), 7);
}
#[test]
fn sessions_json_source_filter() {
    let arr = run_sessions_json(&["--source", "claude"]);
    let sessions = arr.as_array().unwrap();
    assert!(sessions.iter().all(|s| s["source"] == "claude"));
    assert_eq!(sessions.len(), 2);
}

#[test]
fn sessions_json_project_filter() {
    let arr = run_sessions_json(&["--project", "test"]);
    let sessions = arr.as_array().unwrap();
    assert!(!sessions.is_empty());
    assert!(sessions
        .iter()
        .all(|s| s["project"].as_str().unwrap().contains("test")));
}

#[test]
fn sessions_json_limit_offset() {
    let arr = run_sessions_json(&["--limit", "2", "--offset", "1"]);
    let sessions = arr.as_array().unwrap();
    assert_eq!(sessions.len(), 2);
}

#[test]
fn sessions_json_detail() {
    // Find a known session id from fixtures
    let arr = run_sessions_json(&[]);
    let first_id = arr.as_array().unwrap()[0]["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let detail = run_sessions_json(&["--detail", &first_id]);
    assert!(detail.is_object());
    assert_eq!(detail["session_id"].as_str().unwrap(), first_id);
    // Cost is null for claude
    if detail["source"].as_str().unwrap() == "claude" {
        assert_eq!(detail["cost"], serde_json::Value::Null);
    }
}

#[test]
fn sessions_json_sort_desc() {
    let arr = run_sessions_json(&[]);
    let sessions = arr.as_array().unwrap();
    let times: Vec<_> = sessions
        .iter()
        .map(|s| s["started_at"].as_str().unwrap())
        .collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted);
}

// --- report integration tests ---

// Fixture source overrides shared by every report invocation.
fn report_source_flags() -> Vec<String> {
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

// The report TUI is the default when `--output` is absent, so content
// assertions go through a temp file destination; stdout must stay empty.
fn run_report(extra_args: &[&str]) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("report.md");
    run_report_to(&path, extra_args);
    std::fs::read_to_string(&path).unwrap()
}

fn run_report_to(path: &std::path::Path, extra_args: &[&str]) {
    let mut cmd = Command::new(bin());
    cmd.arg("report").arg("--output").arg(path);
    for flag in report_source_flags() {
        cmd.arg(flag);
    }
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper report");
    assert!(
        output.status.success(),
        "llmhelper report exited with {}: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "report must not write to stdout when --output is given"
    );
}

#[test]
fn report_markdown_structure() {
    let out = run_report(&["--last", "30d"]);
    assert!(out.starts_with("# llmhelper report\n"));
    assert!(out.contains("## Totals"));
    assert!(out.contains("## Cost by source"));
    assert!(out.contains("## Usage by source"));
    assert!(out.contains("## Sources"));
    assert!(out.contains("| sessions | messages |"));
    assert!(out.contains("| key | sessions | messages |"));
}

#[test]
fn report_custom_title_heading() {
    let out = run_report(&["--last", "30d", "--title", "Team weekly LLM usage"]);
    assert!(out.starts_with("# Team weekly LLM usage\n"));
}

#[test]
fn report_output_writes_file_and_keeps_stdout_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("report.md");
    run_report_to(&path, &["--last", "30d"]);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.starts_with("# llmhelper report\n"));
    assert!(contents.contains("## Totals"));
    assert!(contents.contains("## Cost by source"));
}

#[test]
fn report_output_unwritable_path_errors() {
    let mut cmd = Command::new(bin());
    cmd.args([
        "report",
        "--last",
        "30d",
        "--output",
        "/nonexistent-dir-xyz-llmhelper/report.md",
    ]);
    cmd.args([
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
    ]);
    let output = cmd.output().expect("failed to run llmhelper report");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to write report"));
}

#[test]
fn report_window_and_filter_echo() {
    let out = run_report(&["--last", "30d", "--project", "proj", "--source", "opencode"]);
    assert!(out.contains("window: last 30d"));
    assert!(out.contains("project=proj"));
    assert!(out.contains("source=opencode"));
}

#[test]
fn report_all_time_window_when_unbounded() {
    let out = run_report(&[]);
    assert!(out.contains("window: all time"));
}

#[test]
fn report_top_collapses_groups() {
    let full = run_report(&["--last", "30d"]);
    assert!(!full.contains("(+ "), "no --top must show every group");
    let capped = run_report(&["--last", "30d", "--top", "1"]);
    assert!(capped.contains("(+ 3 more"));
}

#[test]
fn report_cost_by_source_excludes_claude() {
    let out = run_report(&["--last", "30d"]);
    assert!(out.contains("## Cost by source"));
    let cost_section = out.split("## Usage by").next().unwrap();
    let cost_table = cost_section.split("## Cost by source").nth(1).unwrap();
    assert!(cost_table.contains("opencode"));
    assert!(cost_table.contains("omp"));
    assert!(cost_table.contains("kilo"));
    assert!(!cost_table.contains("claude"));
}

#[test]
fn report_since_last_exclusive_errors() {
    let mut cmd = Command::new(bin());
    cmd.args(["report", "--since", "2026-01-01T00:00:00Z", "--last", "7d"]);
    cmd.args([
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
    ]);
    let output = cmd.output().expect("failed to run llmhelper report");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("mutually exclusive"));
}

#[test]
fn report_top_zero_errors() {
    let mut cmd = Command::new(bin());
    cmd.args(["report", "--top", "0"]);
    cmd.args([
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
    ]);
    let output = cmd.output().expect("failed to run llmhelper report");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--top must be at least 1"));
}

#[test]
fn report_group_by_model_heading() {
    let out = run_report(&["--last", "30d", "--group-by", "model"]);
    assert!(out.contains("## Usage by model"));
}

#[test]
fn report_cost_by_source_order_is_deterministic() {
    let out = run_report(&["--last", "30d"]);
    // Cost table rows should list sources in alphabetical order for
    // deterministic output across runs. We identify data rows as those
    // whose last cell looks like a decimal cost (e.g. " 1.500000 ").
    let lines: Vec<&str> = out.lines().collect();
    let data_rows: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("| ") && l.contains("0.") && l.ends_with(" |"))
        .copied()
        .collect();
    assert!(!data_rows.is_empty(), "expected at least one cost row");
    let sources: Vec<&str> = data_rows
        .iter()
        .map(|l| l.split('|').nth(1).map(|s| s.trim()).unwrap_or(""))
        .collect();
    let mut sorted = sources.clone();
    sorted.sort();
    assert_eq!(sources, sorted);
}

// ---------------------------------------------------------------------------
// `search`
// ---------------------------------------------------------------------------

/// Run `llmhelper search <query> --json` with fixture paths and parse the output.
fn run_search_json(query: &str, extra_args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(bin());
    cmd.args(["search", "--json", query]);
    add_fixture_source_args(&mut cmd);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd.output().expect("failed to run llmhelper search");
    assert!(
        output.status.success(),
        "llmhelper search exited with {}: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap()
}

/// Point every source at its fixture path.
fn add_fixture_source_args(cmd: &mut Command) {
    cmd.arg("--claude-dir").arg(fixture_dir().join("claude"));
    cmd.arg("--opencode-db")
        .arg(fixture_dir().join("opencode").join("opencode.db"));
    cmd.arg("--omp-dir").arg(fixture_dir().join("omp"));
    cmd.arg("--kilo-db")
        .arg(fixture_dir().join("kilo").join("kilo.db"));
}

fn run_search_output(extra_args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin());
    cmd.args(["search", "cache"]);
    add_fixture_source_args(&mut cmd);
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.output().expect("failed to run llmhelper search")
}

#[test]
fn search_sources_panel_lists_every_source_as_ok() {
    let out = run_search_json("token", &[]);
    let panel = out["sources"].as_array().unwrap();
    let names: Vec<&str> = panel.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["claude", "kilo", "omp", "opencode"],
        "every known source should appear in the panel"
    );
    assert!(panel.iter().all(|s| s["status"] == "ok"));
    let messages: u64 = panel.iter().map(|s| s["messages"].as_u64().unwrap()).sum();
    assert!(messages > 0, "fixtures must expose searchable messages");
}

#[test]
fn search_reaches_every_source_that_stores_text() {
    // No single word appears in all four fixtures, so union the corpus over a
    // handful of queries and prove every non-empty source is actually reachable.
    let queries = ["token", "cache", "migration", "fixture", "session"];
    let mut corpus_sources = std::collections::BTreeSet::new();
    let mut hit_sources = std::collections::BTreeSet::new();
    for query in queries {
        let out = run_search_json(query, &[]);
        for s in out["sources"].as_array().unwrap() {
            if s["messages"].as_u64().unwrap_or(0) > 0 {
                corpus_sources.insert(s["name"].as_str().unwrap().to_string());
            }
        }
        for h in out["hits"].as_array().unwrap() {
            hit_sources.insert(h["source"].as_str().unwrap().to_string());
        }
    }
    assert!(!corpus_sources.is_empty());
    assert_eq!(
        hit_sources, corpus_sources,
        "every source with messages must be searchable"
    );
}

#[test]
fn search_hits_are_ranked_by_matches_then_most_recent() {
    let out = run_search_json("the", &[]);
    let hits = out["hits"].as_array().unwrap();
    for w in hits.windows(2) {
        let a = w[0]["matches"].as_u64().unwrap();
        let b = w[1]["matches"].as_u64().unwrap();
        assert!(a >= b, "matches must be non-increasing: {a} then {b}");
        if a == b {
            assert!(
                w[0]["timestamp"].as_str().unwrap() >= w[1]["timestamp"].as_str().unwrap(),
                "ties break newest first"
            );
        }
    }
}

#[test]
fn search_snippet_marks_elision() {
    let out = run_search_json("migration helper", &["--context", "5"]);
    let hits = out["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .any(|h| h["snippet"].as_str().unwrap().starts_with('…')
                || h["snippet"].as_str().unwrap().ends_with('…')),
        "a narrow context window must elide"
    );
}

#[test]
fn search_limit_caps_the_result_set() {
    let out = run_search_json("the", &["--limit", "3"]);
    let hits = out["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(out["limit"].as_u64().unwrap(), 3);
}

#[test]
fn search_case_sensitivity_is_opt_in() {
    let insensitive = run_search_json("CACHE", &[]);
    let sensitive = run_search_json("CACHE", &["--case-sensitive"]);
    assert!(
        !insensitive["hits"].as_array().unwrap().is_empty(),
        "default search is case-insensitive"
    );
    assert!(
        sensitive["hits"].as_array().unwrap().is_empty(),
        "uppercase query should miss lowercase prose when case-sensitive"
    );
    assert_eq!(sensitive["case_sensitive"], true);
}

#[test]
fn search_role_filter_keeps_only_the_requested_role() {
    let out = run_search_json("cache", &["--role", "assistant"]);
    let hits = out["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    assert!(
        hits.iter().all(|h| h["role"] == "assistant"),
        "role filter leaked a non-assistant hit"
    );
    assert_eq!(out["role"].as_str().unwrap(), "assistant");
}

#[test]
fn search_source_filter_restricts_to_one_source() {
    let out = run_search_json("token", &["--source", "kilo"]);
    let hits = out["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    assert!(
        hits.iter().all(|h| h["source"] == "kilo"),
        "source filter leaked another source"
    );
}

#[test]
fn search_project_filter_matches_a_project_substring() {
    let matched = run_search_json("cache", &["--project", "omp-test"]);
    assert!(!matched["hits"].as_array().unwrap().is_empty());
    assert!(matched["hits"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["project"].as_str().unwrap().contains("omp-test")));
    let missed = run_search_json("cache", &["--project", "no-such-project"]);
    assert!(missed["hits"].as_array().unwrap().is_empty());
}

#[test]
fn search_excludes_tool_output_and_synthetic_parts() {
    for query in [
        "HIDDEN",
        "SECRET",
        "SYNTHETIC",
        "IGNORED",
        "TOOL_RESULT_MARKER",
    ] {
        let out = run_search_json(query, &[]);
        assert!(
            out["hits"].as_array().unwrap().is_empty(),
            "{query}: non-prose content must not be searchable"
        );
        // The exclusion is per-message, not per-source: every source still
        // reports itself as ok rather than erroring out.
        assert!(out["sources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] == "ok"));
    }
}

#[test]
fn search_no_searchable_text_is_honest_zero_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, model TEXT);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT, time_created INTEGER);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT);
         INSERT INTO session VALUES ('ses_empty', '/home/hunter/projects/empty', 'auto');
         INSERT INTO message VALUES
           ('m1', 'ses_empty', '{}', 1756400000000),
           ('m2', 'ses_empty', '{}', 1756400060000);
         INSERT INTO part VALUES
           ('p1', 'm1', '{\"type\":\"tool\",\"tool\":\"bash\",\"state\":{}}'),
           ('p2', 'm2', '{\"type\":\"text\",\"text\":\"\",\"synthetic\":true}');",
    )
    .unwrap();
    drop(conn);

    // Scope the other sources to empty paths so this only touches the temp DB.
    let mut cmd = Command::new(bin());
    cmd.args(["search", "--json", "whatever"]);
    cmd.arg("--opencode-db").arg(&db);
    cmd.arg("--claude-dir").arg(dir.path().join("no-claude"));
    cmd.arg("--omp-dir").arg(dir.path().join("no-omp"));
    cmd.arg("--kilo-db").arg(dir.path().join("no-kilo.db"));
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out: serde_json::Value =
        serde_json::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert!(out["hits"].as_array().unwrap().is_empty());
    let panel = out["sources"].as_array().unwrap();
    let opencode = panel.iter().find(|s| s["name"] == "opencode").unwrap();
    assert_eq!(opencode["status"], "ok");
    assert_eq!(opencode["messages"].as_u64().unwrap(), 0);
}

#[test]
fn search_since_in_the_future_returns_nothing() {
    let before = run_search_json("cache", &["--since", "2000-01-01T00:00:00Z"]);
    let after = run_search_json("cache", &["--since", "2030-01-01T00:00:00Z"]);
    assert!(!before["hits"].as_array().unwrap().is_empty());
    assert!(after["hits"].as_array().unwrap().is_empty());
}

#[test]
fn search_sources_panel_counts_the_filtered_corpus() {
    // The panel must say what was searched, not what was loaded: excluding
    // three projects must zero out the sources that own them.
    let out = run_search_json("cache", &["--project", "omp-test"]);
    let panel = out["sources"].as_array().unwrap();
    let omp = panel.iter().find(|s| s["name"] == "omp").unwrap();
    assert!(omp["messages"].as_u64().unwrap() > 0);
    for s in panel.iter().filter(|s| s["name"] != "omp") {
        assert_eq!(
            s["messages"].as_u64().unwrap(),
            0,
            "{} holds no messages in the filtered corpus",
            s["name"]
        );
        assert_eq!(s["status"], "ok");
    }
}

#[test]
fn search_csv_output_has_expected_header() {
    let output = run_search_output(&["--csv", "--limit", "1"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let header = stdout.lines().next().unwrap();
    assert_eq!(
        header,
        "source,session_id,project,model,role,timestamp,matches,snippet"
    );
}

#[test]
fn search_text_output_lists_hits_without_a_tty() {
    let output = run_search_output(&["--text", "--limit", "1"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines.len() >= 2,
        "expected an indexed hit plus a metadata line"
    );
    assert!(lines[0].starts_with("[1]"));
}

#[test]
fn search_rejects_empty_query() {
    let output = Command::new(bin())
        .args(["search", "   ", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("query must not be empty"));
}

#[test]
fn search_rejects_zero_limit() {
    let output = Command::new(bin())
        .args(["search", "x", "--limit", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--limit must be at least 1"));
}

#[test]
fn search_rejects_json_and_csv_together() {
    let output = Command::new(bin())
        .args(["search", "x", "--json", "--csv"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn search_rejects_since_and_last_together() {
    let output = Command::new(bin())
        .args([
            "search",
            "x",
            "--since",
            "2000-01-01T00:00:00Z",
            "--last",
            "1d",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

// --- Budget annotation -------------------------------------------------------

/// The opencode fixture records 0.17 total Cost; omp records 1.5. Claude
/// records none. These thresholds therefore produce `over`, `over`, and
/// `not measured` respectively once the window is wide enough to see them.
fn run_report_raw(extra_args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin());
    cmd.args([
        "report",
        "--output",
        "/dev/stdout",
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
    ]);
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.output().expect("failed to run llmhelper report")
}

#[test]
fn report_without_budgets_omits_budget_section() {
    let out = run_report(&["--last", "30d"]);
    assert!(!out.contains("## Budget"));
}

#[test]
fn report_budget_section_reports_over() {
    let out = run_report(&[
        "--last",
        "4000d",
        "--budget",
        "opencode:0.01",
        "--budget-window",
        "4000d",
    ]);
    assert!(out.contains("## Budget"), "{out}");
    assert!(
        out.contains("| cli:opencode | opencode | 4000d | 0.170000 | 0.010000 | over |"),
        "{out}"
    );
}

#[test]
fn report_budget_section_reports_not_measured_for_claude() {
    let out = run_report(&[
        "--last",
        "4000d",
        "--budget",
        "claude:5.00",
        "--budget-window",
        "4000d",
    ]);
    assert!(
        out.contains("| cli:claude | claude | 4000d | — | 5.000000 | not measured |"),
        "{out}"
    );
    assert!(
        !out.contains("| cli:claude | claude | 4000d | 0.000000 |"),
        "{out}"
    );
}

#[test]
fn report_budget_sits_after_cost_and_before_usage() {
    let out = run_report(&[
        "--last",
        "4000d",
        "--budget",
        "opencode:0.01",
        "--budget-window",
        "4000d",
    ]);
    let cost = out.find("## Cost by source").unwrap();
    let budget = out.find("## Budget").unwrap();
    let usage = out.find("## Usage by").unwrap();
    assert!(cost < budget && budget < usage, "{out}");
}

#[test]
fn report_unclipped_budget_has_no_caveat() {
    let out = run_report(&[
        "--last",
        "4000d",
        "--budget",
        "opencode:0.01",
        "--budget-window",
        "4000d",
    ]);
    assert!(
        !out.contains("measured over the loaded range only"),
        "{out}"
    );
}

#[test]
fn report_wider_budget_window_emits_caveat() {
    let out = run_report(&[
        "--last",
        "2d",
        "--budget",
        "opencode:0.01",
        "--budget-window",
        "4000d",
    ]);
    assert!(out.contains("cli:opencode: only records from"), "{out}");
}

#[test]
fn budget_name_selects_one_configured_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    fs::write(
        &config,
        "[budget.oc]\nsource = \"opencode\"\nwindow = \"4000d\"\nmax_cost = 0.01\n\
         [budget.omp]\nsource = \"omp\"\nwindow = \"4000d\"\nmax_cost = 0.01\n",
    )
    .unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    let path = tmp2.path().join("report.md");
    let mut cmd = Command::new(bin());
    cmd.args(["report", "--output"]).arg(&path);
    cmd.args(["--config"]).arg(&config);
    for flag in report_source_flags() {
        cmd.arg(flag);
    }
    cmd.args(["--last", "4000d", "--budget-name", "oc"]);
    let output = cmd.output().expect("failed to run report");
    assert!(
        output.status.success(),
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = fs::read_to_string(&path).unwrap();
    // Scope the assertions to the Budget section: "omp" also appears in the
    // Usage-by-source table below it.
    let budget_section = out
        .split("## Budget")
        .nth(1)
        .and_then(|s| s.split("## Usage by").next())
        .unwrap_or("");
    assert!(budget_section.contains("| oc | opencode |"), "{out}");
    assert!(!budget_section.contains("| omp |"), "{out}");
}

#[test]
fn unknown_budget_name_errors_listing_configured_names() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    fs::write(
        &config,
        "[budget.oc]\nsource = \"opencode\"\nwindow = \"1d\"\nmax_cost = 5.0\n",
    )
    .unwrap();
    let out = run_report_raw(&[
        "--config",
        config.to_str().unwrap(),
        "--budget-name",
        "nope",
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown --budget-name 'nope'"), "{stderr}");
    assert!(stderr.contains("oc"), "{stderr}");
}

#[test]
fn bad_budget_syntax_errors() {
    let out = run_report_raw(&["--budget", "nocolon"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("expected <source>:<amount>"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn budget_with_unknown_source_errors() {
    let out = run_report_raw(&["--budget", "bogus:5"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown source 'bogus'"));
}

#[test]
fn budget_with_non_positive_amount_errors() {
    let out = run_report_raw(&["--budget", "opencode:0"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("positive finite number"));
}

#[test]
fn bad_budget_window_errors() {
    let out = run_report_raw(&["--budget", "opencode:5", "--budget-window", "fortnight"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid --budget-window"));
}

#[test]
fn usage_without_budgets_is_unaffected() {
    // A no-budget run must still parse and emit the usual JSON shape.
    let json = run_usage_json(&[]);
    assert!(json["sources"].is_array());
}

#[test]
fn usage_rejects_bad_budget_syntax() {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json", "--budget", "nocolon"]);
    for flag in report_source_flags() {
        cmd.arg(flag);
    }
    let output = cmd.output().expect("failed to run usage");
    assert!(!output.status.success());
}

#[test]
#[cfg(unix)]
fn truncating_pipe_does_not_panic() {
    // A downstream reader that closes early (`| head -1`) must not turn the
    // first failed stdout write into a panic-with-stack-trace. The binary
    // should die from SIGPIPE (or exit cleanly) with no "panicked" message.
    let bin = bin();
    let script = format!(
        "{} export --last 3650d --claude-dir {} --opencode-db {} --omp-dir {} --kilo-db {} 2>/tmp/llmhelper_pipe_stderr.txt | head -1 >/dev/null; cat /tmp/llmhelper_pipe_stderr.txt",
        bin.display(),
        fixture_dir().join("claude").display(),
        fixture_dir().join("opencode").join("opencode.db").display(),
        fixture_dir().join("omp").display(),
        fixture_dir().join("kilo").join("kilo.db").display(),
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("failed to run pipeline");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stderr, String::from_utf8_lossy(&output.stdout));
    assert!(
        !combined.contains("panicked") && !combined.contains("Broken pipe"),
        "truncating the output pipe should not panic; saw: {combined}"
    );
}

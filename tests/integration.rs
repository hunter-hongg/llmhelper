//! Integration tests driven via the real `usage --json` CLI binary.
//! This is the single testing seam from the spec.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use chrono::Utc;

/// Path to the release binary.
fn bin() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_BIN_EXE_llmhelper"));
    p
}

/// Build the fixture directory path.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// Run `llmhelper usage --json` with fixture paths and parse the output.
fn run_usage_json(extra_args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(bin());
    cmd.args(["usage", "--json"]);
    cmd.args([
        "--claude-dir",
        fixture_dir().join("claude").to_str().unwrap(),
        "--opencode-db",
        fixture_dir().join("opencode").join("opencode.db").to_str().unwrap(),
        "--omp-dir",
        fixture_dir().join("omp").to_str().unwrap(),
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

#[test]
fn json_output_total_session_count() {
    let json = run_usage_json(&[]);
    let sources = json["sources"].as_array().unwrap();
    let total_records: usize = sources.iter().map(|s| s["records"].as_u64().unwrap() as usize).sum();
    // Claude: 2 sessions (session-a + session-b)
    // OpenCode: 2 sessions (ses_fix_001 + ses_fix_002)
    // OMP: 1 session (01fixomp)
    assert_eq!(total_records, 5);
}

#[test]
fn json_output_claude_cost_is_null() {
    let json = run_usage_json(&["--source", "claude"]);
    let groups = json["groups"].as_array().unwrap();
    for g in groups {
        assert_eq!(g["cost"], serde_json::Value::Null, "Claude cost must be null");
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
    // All three sources have projects under /home/hunter
    assert_eq!(groups.len(), 3);
}

#[test]
fn json_output_group_by_model() {
    let json = run_usage_json(&["--group-by", "model"]);
    let groups = json["groups"].as_array().unwrap();
    let auto = groups.iter().find(|g| g["key"] == "auto").unwrap();
    assert_eq!(auto["sessions"], 2); // claude session-b + omp (both raw model "auto")
    let bp = groups.iter().find(|g| g["key"] == "big-pickle").unwrap();
    assert_eq!(bp["sessions"], 1);
}

#[test]
fn json_output_last_filter() {
    // --last 1d should include recent fixture sessions
    let json = run_usage_json(&["--last", "1d"]);
    let sources = json["sources"].as_array().unwrap();
    let total: usize = sources.iter().map(|s| s["records"].as_u64().unwrap() as usize).sum();
    // Fixture timestamps may be in the past; just verify it runs.
    let _ = total;

    // --last 100y should include everything
    let json = run_usage_json(&["--last", "36500d"]);
    let sources = json["sources"].as_array().unwrap();
    let total_all: usize = sources.iter().map(|s| s["records"].as_u64().unwrap() as usize).sum();
    assert_eq!(total_all, 5);
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
        fixture_dir().join("opencode").join("opencode.db").to_str().unwrap(),
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
        fixture_dir().join("opencode").join("opencode.db").to_str().unwrap(),
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
    cmd.args(["usage", "--json", "--since", "2025-01-01T00:00:00Z", "--last", "7d"]);
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

/// Helper: run `llmhelper diff --json` with ephemeral fixtures.
/// Timestamps are computed relative to `Utc::now()` at test-run time, so
/// they reliably fall into the prev / curr windows regardless of wall clock.
/// Windows use `--last {window}s --prev {window}s`.
fn run_diff_json(
    fixture_base: &std::path::Path,
    window: u64,      // seconds for --last / --prev
    extra_args: &[&str],
) -> serde_json::Value {
    // Ensure all three source dirs exist (even if empty) so discover_sources
    // does NOT fall back to the real user's ~/.local/share/opencode/ or
    // ~/.omp/agent/sessions/ and pollute the diff with unrelated rows.
    let claude_dir = fixture_base.join("claude");
    let omp_dir = fixture_base.join("omp");
    let opencode_dir = fixture_base.join("opencode");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::create_dir_all(&omp_dir).unwrap();
    fs::create_dir_all(&opencode_dir).unwrap();
    fs::write(opencode_dir.join("opencode.db"), "").unwrap(); // empty sqlite stub

    let mut cmd = Command::new(bin());
    cmd.args(["diff", "--json"]);
    let dur = format!("{}s", window);
    cmd.arg("--last").arg(&dur);
    cmd.arg("--prev").arg(&dur);
    cmd.arg("--claude-dir").arg(claude_dir.to_str().unwrap());
    cmd.arg("--omp-dir").arg(omp_dir.to_str().unwrap());
    cmd.arg("--opencode-db").arg(opencode_dir.join("opencode.db").to_str().unwrap());
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

fn make_session(base: &std::path::Path, project_dir: &str, file: &str, ts: &str, input: u64, output: u64) {
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
    make_session(base, "-home-hunter-proj-growth", "session-prev.jsonl", &prev_ts, 100, 50);
    make_session(base, "-home-hunter-proj-growth", "session-curr.jsonl", &curr_ts, 200, 100);
}

/// A pair of Claude sessions with a token shrink.
fn build_shrink_fixture(base: &std::path::Path) {
    let prev_ts = diff_prev_ts();
    let curr_ts = diff_curr_ts();
    make_session(base, "-home-hunter-proj-shrink", "session-prev.jsonl", &prev_ts, 200, 100);
    make_session(base, "-home-hunter-proj-shrink", "session-curr.jsonl", &curr_ts, 100, 50);
}

/// One Claude session in the current window only.
fn build_new_fixture(base: &std::path::Path) {
    let curr_ts = diff_curr_ts();
    make_session(base, "-home-hunter-proj-new", "session-curr.jsonl", &curr_ts, 50, 25);
}

/// One Claude session in the previous window only.
fn build_removed_fixture(base: &std::path::Path) {
    let prev_ts = diff_prev_ts();
    make_session(base, "-home-hunter-proj-removed", "session-prev.jsonl", &prev_ts, 50, 25);
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
    assert!(rows[0]["delta"]["pct"]["input"].is_null(), "pct must be null when prev is zero");
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
    cmd.arg("--claude-dir").arg(dir.path().join("claude").to_str().unwrap());
    cmd.arg("--omp-dir").arg(dir.path().join("omp").to_str().unwrap());
    cmd.arg("--opencode-db").arg(opencode_dir.join("opencode.db").to_str().unwrap());
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
    cmd.arg("--claude-dir").arg(dir.path().join("claude").to_str().unwrap());
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
    cmd.arg("--claude-dir").arg(dir.path().join("claude").to_str().unwrap());
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
    cmd.arg("--claude-dir").arg(dir.path().join("claude").to_str().unwrap());
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
    cmd.arg("--claude-dir").arg(dir.path().join("claude").to_str().unwrap());
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
    cmd.arg("--claude-dir").arg(dir.path().join("claude").to_str().unwrap());
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

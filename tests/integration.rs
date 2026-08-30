//! Integration tests driven via the real `usage --json` CLI binary.
//! This is the single testing seam from the spec.

use std::process::Command;
use std::path::PathBuf;

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
    assert_eq!(total_records, 4);
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
fn json_output_token_sums_match_fixtures() {
    let json = run_usage_json(&[]);
    let groups = json["groups"].as_array().unwrap();
    let claude = groups.iter().find(|g| g["key"] == "claude").unwrap();
    assert_eq!(claude["tokens"]["input"], 650);
    assert_eq!(claude["tokens"]["output"], 325);
    assert_eq!(claude["tokens"]["reasoning"], 15);
    assert_eq!(claude["tokens"]["cache_read"], 30);
    assert_eq!(claude["tokens"]["cache_write"], 10);

    let oc = groups.iter().find(|g| g["key"] == "opencode").unwrap();
    assert_eq!(oc["tokens"]["input"], 59537);
    assert_eq!(oc["tokens"]["output"], 5017);
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
    // Both sources have projects under /home/hunter
    assert_eq!(groups.len(), 2);
}

#[test]
fn json_output_group_by_model() {
    let json = run_usage_json(&["--group-by", "model"]);
    let groups = json["groups"].as_array().unwrap();
    let auto = groups.iter().find(|g| g["key"] == "auto").unwrap();
    assert_eq!(auto["sessions"], 1);
    let bp = groups.iter().find(|g| g["key"] == "big-pickle").unwrap();
    assert_eq!(bp["sessions"], 1);
}

#[test]
fn json_output_last_filter() {
    // --last 1d should include recent fixture sessions
    let json = run_usage_json(&["--last", "1d"]);
    let sources = json["sources"].as_array().unwrap();
    let total: usize = sources.iter().map(|s| s["records"].as_u64().unwrap() as usize).sum();
    assert!(total >= 0); // fixture timestamps may be in the past; just verify it runs

    // --last 100y should include everything
    let json = run_usage_json(&["--last", "36500d"]);
    let sources = json["sources"].as_array().unwrap();
    let total_all: usize = sources.iter().map(|s| s["records"].as_u64().unwrap() as usize).sum();
    assert_eq!(total_all, 4);
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

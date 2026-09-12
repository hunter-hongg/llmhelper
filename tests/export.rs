//! Integration tests for `export`, driven through the real CLI binary against
//! the shared fixture tree. This is the CLI seam from spec 0014.

use std::path::PathBuf;
use std::process::{Command, Output};

/// Path to the test binary.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_llmhelper"))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Run `llmhelper export` with the fixture source paths plus `extra_args`.
fn run_export(extra_args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.arg("export");
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
    cmd.output().expect("failed to run llmhelper")
}

/// Run and require success, returning stdout.
fn stdout_ok(extra_args: &[&str]) -> String {
    let output = run_export(extra_args);
    assert!(
        output.status.success(),
        "export exited with {}: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn export_jsonl_emits_one_object_per_line() {
    let out = stdout_ok(&[]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(!lines.is_empty(), "expected records from fixtures");
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v.is_object());
        assert!(v["source"].is_string());
        assert!(v["session_id"].is_string());
    }
}

#[test]
fn export_jsonl_count_matches_sessions_command() {
    let export_out = stdout_ok(&[]);
    let export_lines = export_out.lines().count();

    // `sessions --json` emits the same Record set as a pretty array.
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
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let sessions: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(export_lines, sessions.as_array().unwrap().len());
}

#[test]
fn export_json_is_one_parseable_array() {
    let out = stdout_ok(&["--format", "json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v.is_array());
    assert!(!v.as_array().unwrap().is_empty());
}

#[test]
fn export_csv_has_header_and_rows() {
    let out = stdout_ok(&["--format", "csv"]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() >= 2, "header plus at least one row");
    assert_eq!(
        lines[0],
        "source,session_id,project,model,agent,started_at,ended_at,messages,input,output,cache_read,cache_write,cost"
    );
    for line in &lines[1..] {
        assert_eq!(line.split(',').count(), 13);
    }
}

#[test]
fn export_tsv_uses_tab_delimiter() {
    let out = stdout_ok(&["--format", "tsv"]);
    let header = out.lines().next().unwrap();
    assert!(header.contains('\t'));
    assert_eq!(header.split('\t').count(), 13);
}

#[test]
fn export_fields_narrows_and_reorders_jsonl() {
    let out = stdout_ok(&["--fields", "input,source", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let first = v.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(first.len(), 2);
    assert!(first.contains_key("input"));
    assert!(first.contains_key("source"));
    // Raw text order check: "input" must appear before "source".
    let line = out.lines().find(|l| l.contains("\"input\"")).unwrap();
    assert!(line.find("\"input\"").unwrap() < line.find("\"source\"").unwrap());
}

#[test]
fn export_fields_repeated_accumulates() {
    let out = stdout_ok(&[
        "--fields", "source", "--fields", "input", "--format", "json",
    ]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let first = v.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(first.len(), 2);
    assert!(first.contains_key("source"));
    assert!(first.contains_key("input"));
}

#[test]
fn export_unknown_field_exits_one() {
    let output = run_export(&["--fields", "bogus"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bogus"), "got: {stderr}");
    assert!(stderr.contains("valid fields"), "got: {stderr}");
}

#[test]
fn export_since_and_last_are_mutually_exclusive() {
    let output = run_export(&["--since", "2026-01-01T00:00:00Z", "--last", "7d"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("mutually exclusive"));
}

#[test]
fn export_source_filter_narrows() {
    let all = stdout_ok(&[]).lines().count();
    let claude_out = stdout_ok(&["--source", "claude"]);
    let claude = claude_out.lines().count();
    assert!(claude > 0);
    assert!(claude < all, "claude filter should exclude other sources");
    for line in claude_out.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["source"], "claude");
    }
}

#[test]
fn export_since_future_window_is_empty_and_exits_zero() {
    let output = run_export(&["--since", "2999-01-01T00:00:00Z"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn export_cost_null_for_claude_and_present_for_omp() {
    let out = stdout_ok(&[]);
    let mut saw_claude = false;
    let mut saw_omp_cost = false;
    for line in out.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v["source"] == "claude" {
            saw_claude = true;
            assert!(v["cost"].is_null(), "claude records no cost");
        }
        if v["source"] == "omp" && !v["cost"].is_null() {
            saw_omp_cost = true;
        }
    }
    assert!(saw_claude && saw_omp_cost);
}

// ---------------------------------------------------------------------------
// `export --messages` — transcript-level export (spec 0016)
// ---------------------------------------------------------------------------

/// Run `export --messages ...` with the fixture source paths plus extra args.
fn stdout_messages_ok(extra_args: &[&str]) -> String {
    let mut args = vec!["--messages"];
    args.extend_from_slice(extra_args);
    stdout_ok(&args)
}

#[test]
fn messages_emits_one_row_per_message_across_sources() {
    let out = stdout_messages_ok(&[]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(!lines.is_empty(), "expected messages from fixtures");

    let mut sources = std::collections::BTreeSet::new();
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v.is_object());
        assert!(v["source"].is_string());
        assert!(v["session_id"].is_string());
        assert!(v["role"].is_string());
        assert!(v["text"].is_string());
        sources.insert(v["source"].as_str().unwrap().to_string());
    }
    // Every Source that stores text must contribute at least one message.
    for name in ["claude", "opencode", "omp", "kilo"] {
        assert!(sources.contains(name), "missing source {name}");
    }
}

#[test]
fn messages_role_filter_narrows_to_that_role() {
    let all = stdout_messages_ok(&[]).lines().count();
    let assistants = stdout_messages_ok(&["--role", "assistant"]);
    let count = assistants.lines().count();
    assert!(count > 0, "expected assistant messages");
    assert!(count < all, "role filter should exclude other roles");
    for line in assistants.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["role"], "assistant");
    }
}

#[test]
fn messages_role_filter_is_case_insensitive() {
    let lower = stdout_messages_ok(&["--role", "assistant"]).lines().count();
    let upper = stdout_messages_ok(&["--role", "ASSISTANT"]).lines().count();
    assert_eq!(lower, upper);
}

#[test]
fn messages_role_without_messages_flag_exits_one() {
    let output = run_export(&["--role", "user"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--role"), "got: {stderr}");
    assert!(stderr.contains("--messages"), "got: {stderr}");
}

#[test]
fn messages_source_filter_narrows() {
    let claude = stdout_messages_ok(&["--source", "claude"]);
    assert!(claude.lines().count() > 0);
    for line in claude.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["source"], "claude");
    }
}

#[test]
fn messages_project_filter_narrows() {
    let out = stdout_messages_ok(&["--project", "omp-test"]);
    assert!(out.lines().count() > 0, "expected omp-test messages");
    for line in out.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(
            v["project"].as_str().unwrap().contains("omp-test"),
            "got: {}",
            v["project"]
        );
    }
}

#[test]
fn messages_last_filter_fails_closed_for_old_and_untimed_rows() {
    // Fixtures are dated far in the past, so a 1-day window is empty.
    let out = stdout_messages_ok(&["--last", "1d"]);
    assert_eq!(out.lines().count(), 0);
    // A wide window admits the whole corpus.
    let wide = stdout_messages_ok(&["--last", "3650d"]);
    assert!(wide.lines().count() > 0);
}

#[test]
fn messages_fields_narrows_and_reorders() {
    let out = stdout_messages_ok(&["--fields", "role,text", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let first = v.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(first.len(), 2);
    assert!(first.contains_key("role"));
    assert!(first.contains_key("text"));
    let line = out.lines().find(|l| l.contains("\"role\"")).unwrap();
    assert!(line.find("\"role\"").unwrap() < line.find("\"text\"").unwrap());
}

#[test]
fn messages_json_is_one_parseable_array() {
    let out = stdout_messages_ok(&["--format", "json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v.is_array());
    assert!(!v.as_array().unwrap().is_empty());
}

#[test]
fn messages_csv_has_message_header() {
    let out = stdout_messages_ok(&["--format", "csv"]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() >= 2);
    assert_eq!(
        lines[0],
        "source,session_id,project,model,role,timestamp,text"
    );
}

#[test]
fn messages_record_only_field_exits_one_naming_it() {
    let output = run_export(&["--messages", "--fields", "cost"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cost"), "got: {stderr}");
    assert!(stderr.contains("--messages"), "got: {stderr}");
}

#[test]
fn messages_absent_model_and_timestamp_are_null_not_zero() {
    let out = stdout_messages_ok(&["--fields", "model,timestamp,text"]);
    let mut saw_null_model = false;
    for line in out.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        // A user message commonly has no model; assert null, never "0".
        if v["model"].is_null() {
            saw_null_model = true;
        }
        assert!(!v["model"].is_number() || v["model"].as_f64() != Some(0.0));
    }
    assert!(
        saw_null_model,
        "fixtures must include a message without a model"
    );
}

#[test]
fn messages_zero_match_exits_zero_with_empty_shape() {
    let output = run_export(&["--messages", "--role", "no-such-role"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "jsonl of nothing writes nothing");
}

#[test]
fn messages_output_is_deterministic_across_runs() {
    let a = stdout_messages_ok(&[]);
    let b = stdout_messages_ok(&[]);
    assert_eq!(a, b, "two identical exports must be byte-identical");
}

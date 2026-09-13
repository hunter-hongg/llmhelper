//! Integration tests for `compare`, driven through the real CLI binary against
//! the shared fixture tree. This is the CLI seam from spec 0023.
//!
//! The ranking, share arithmetic, and `(others)` fold are unit-tested in
//! `src/compare.rs` with hand-built groups. These tests assert only the
//! **external** behavior: that the CLI orders rows, folds the tail, renders the
//! three formats, errors on the flag conflicts, and treats an empty rank as a
//! valid (exit 0) result.
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
fn absent_paths(base: &Path) -> Vec<String> {
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
    cmd.arg("compare");
    cmd.args(paths);
    cmd.args(extra_args);
    cmd.output().expect("failed to run llmhelper compare")
}

fn run(extra_args: &[&str]) -> Output {
    run_with(&fixture_paths(), extra_args)
}

/// A unique scratch dir per test, so absent-path tests cannot collide.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("llmhelper-compare-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn json_of(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "compare exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("compare --json did not emit valid JSON")
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "compare exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A wide window that covers the fixtures' fixed 2026-08-27/28 timestamps.
const WIDE: &[&str] = &["--last", "365d"];

#[test]
fn json_rows_are_ranked_by_tokens_descending() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--json"]);
    let json = json_of(&run(&args));
    let rows = json["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "fixtures must yield groups");

    // rank is 1..n with no gaps, and tokens are non-increasing.
    let mut prev = u64::MAX;
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["rank"].as_u64().unwrap(), (i + 1) as u64);
        let total = row["tokens"]["input"].as_u64().unwrap()
            + row["tokens"]["output"].as_u64().unwrap()
            + row["tokens"]["cache_read"].as_u64().unwrap()
            + row["tokens"]["cache_write"].as_u64().unwrap();
        assert!(total <= prev, "tokens are not descending: {total} > {prev}");
        prev = total;
    }

    assert_eq!(json["group_by"], "source");
    assert_eq!(json["sort_by"], "tokens");
    assert!(json["grand_tokens"].is_u64());
}

#[test]
fn sort_by_cost_puts_a_costless_group_last() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--sort-by", "cost", "--json"]);
    let json = json_of(&run(&args));
    let rows = json["rows"].as_array().unwrap();
    assert!(rows.len() > 1);

    // Cost-bearing groups precede the cost-less claude row, and a cost-less row
    // reports share null, never 0.
    let last = rows.last().unwrap();
    let mut seen_null_cost = false;
    let mut seen_cost = false;
    for row in rows {
        if row["cost"].is_null() {
            assert_eq!(row["share_pct"], serde_json::json!(null));
            seen_null_cost = true;
        } else {
            assert!(row["cost"].as_f64().unwrap() >= 0.0);
            assert!(
                !seen_null_cost,
                "a cost-bearing row followed a cost-less one"
            );
            seen_cost = true;
        }
    }
    assert!(seen_cost && seen_null_cost, "fixtures should exercise both");
    assert!(last["cost"].is_null());
}

#[test]
fn top_n_folds_the_tail_into_an_others_row() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--top", "2", "--json"]);
    let json = json_of(&run(&args));
    let rows = json["rows"].as_array().unwrap();
    // Four fixture sources, top 2 ⇒ 2 real rows + 1 fold.
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["key"], "opencode");
    assert_eq!(rows[2]["key"], "(others)");
    assert_eq!(rows[2]["is_others"], true);

    // The fold's tokens equal the sum of the rows it swallowed.
    let mut expected_total = 0u64;
    let mut all_args: Vec<String> = fixture_paths();
    all_args.extend([
        "--last".into(),
        "365d".into(),
        "--group-by".into(),
        "source".into(),
        "--json".into(),
    ]);
    let all = json_of(&run_with(&all_args, &[]));
    for row in all["rows"].as_array().unwrap() {
        if row["key"] != "opencode" && row["key"] != "kilo" {
            let t = row["tokens"].as_object().unwrap();
            expected_total += t.values().map(|v| v.as_u64().unwrap()).sum::<u64>();
        }
    }
    let others = rows[2]["tokens"].as_object().unwrap();
    let got: u64 = others.values().map(|v| v.as_u64().unwrap()).sum();
    assert_eq!(got, expected_total);
}

#[test]
fn top_at_or_above_count_emits_no_others_row() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--top", "99", "--json"]);
    let json = json_of(&run(&args));
    let rows = json["rows"].as_array().unwrap();
    assert!(rows.iter().all(|r| r["is_others"] == false));
    assert_eq!(rows.len(), 4);
}

#[test]
fn group_by_changes_the_visible_keys() {
    let mut by_source: Vec<&str> = WIDE.to_vec();
    by_source.extend(["--group-by", "source", "--json"]);
    let s = json_of(&run(&by_source));
    assert_eq!(s["group_by"], "source");

    let mut by_project: Vec<&str> = WIDE.to_vec();
    by_project.extend(["--group-by", "project", "--json"]);
    let p = json_of(&run(&by_project));
    assert_eq!(p["group_by"], "project");
    // The two dimensions slice the same records differently.
    assert_ne!(s["rows"], p["rows"]);
}

#[test]
fn empty_result_is_exit_zero_and_reports_diagnostics() {
    let base = scratch("empty");
    let paths = absent_paths(&base);
    let output = run_with(
        &paths,
        &["--last", "1h", "--project", "/nonexistent", "--json"],
    );
    assert!(
        output.status.success(),
        "an empty compare must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["rows"].as_array().unwrap().len(), 0);
    // Spec 0020: diagnostics appear for an empty result even without --explain.
    assert!(json["diagnostics"].is_object());
    assert_eq!(json["diagnostics"]["matched"], 0);
}

#[test]
fn empty_table_prints_a_header_and_no_rows() {
    let base = scratch("empty-table");
    let paths = absent_paths(&base);
    let output = run_with(&paths, &["--last", "1h", "--project", "/nonexistent"]);
    let stdout = stdout_of(&output);
    // The header names the dimension and the row count (zero).
    assert!(stdout.contains("rows: 0"), "header missing: {stdout}");
    // No data row: the only lines are the sources strip, the header, the column
    // header, and the blank between them.
    let data_lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with(' ') && !l.trim().starts_with("rank"))
        .collect();
    assert!(
        data_lines.is_empty(),
        "an empty table must have no data rows: {data_lines:?}"
    );
}

#[test]
fn csv_body_is_header_plus_rows_and_explain_goes_to_stderr() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--csv"]);
    let output = run(&args);
    let stdout = stdout_of(&output);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    // Header + one line per source.
    assert_eq!(lines.len(), 5, "csv body was: {stdout}");
    assert!(lines[0].starts_with("rank,key,is_others"));

    // With --explain the funnel lands on stderr and the stdout body is
    // unchanged, so a strict CSV parser still reads the table.
    let mut explained: Vec<&str> = WIDE.to_vec();
    explained.extend(["--group-by", "source", "--csv", "--explain"]);
    let output = run(&explained);
    let stdout_explained = stdout_of(&output);
    assert_eq!(
        stdout_explained, stdout,
        "--explain must not touch the CSV body"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("filters:"),
        "--explain funnel should go to stderr"
    );
}

#[test]
fn json_and_csv_are_mutually_exclusive() {
    let output = run(&["--last", "30d", "--json", "--csv"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn invalid_sort_by_is_a_clap_error() {
    let output = run(&["--last", "30d", "--sort-by", "bananas"]);
    assert!(!output.status.success());
}

#[test]
fn deterministic_output_across_runs() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "project", "--json"]);
    let first = stdout_of(&run(&args));
    let second = stdout_of(&run(&args));
    assert_eq!(first, second, "compare output must be byte-stable");
}

#[test]
fn json_non_empty_run_omits_diagnostics_without_explain() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--json"]);
    let json = json_of(&run(&args));
    // Spec 0020: a non-empty result carries no diagnostics key unless asked.
    assert!(json.get("diagnostics").is_none());
}

#[test]
fn json_explain_present_when_requested() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--json", "--explain"]);
    let json = json_of(&run(&args));
    assert!(json["diagnostics"].is_object());
}

// --- Budget annotation (spec 0024) ---
//
// Fixture costs: opencode 0.17, kilo 1.5, omp 1.5, claude none. A budget window
// of 365d matches the `WIDE` window, so the fixtures' fixed 2026-08-27 dates
// fall inside it.

/// The fixtures' opencode spend (0.17) exceeds 0.10; kilo (1.5) is under 5; and
/// claude records no cost at all, so it is `not measured` rather than `ok`.
fn budget_args() -> Vec<&'static str> {
    vec![
        "--budget",
        "opencode:0.10",
        "--budget",
        "kilo:5",
        "--budget",
        "claude:1",
        "--budget-window",
        "365d",
    ]
}

#[test]
fn over_budget_source_is_marked_and_others_are_not() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source"]);
    args.extend(budget_args());
    let stdout = stdout_of(&run(&args));
    // The over Source's row carries the glyph; the others do not.
    let opencode = stdout
        .lines()
        .find(|l| l.contains("opencode") && !l.contains("sources:"))
        .expect("opencode row");
    assert!(opencode.contains('⚠'), "opencode row was: {opencode}");
    let kilo = stdout
        .lines()
        .find(|l| l.contains("kilo") && !l.contains("sources:"))
        .expect("kilo row");
    assert!(!kilo.contains('⚠'), "kilo row was: {kilo}");
    // The header states the situation, reusing usage's wording.
    assert!(stdout.contains("budgets: 1 over"), "header was: {stdout}");
}

#[test]
fn no_budget_output_is_byte_identical_to_budgetless_run() {
    // The budget layer must vanish entirely when no budgets are configured: no
    // glyph, no header count, no JSON field, no CSV column. This is asserted on
    // `--group-by source`, a dimension that cannot produce a mixed group — see
    // `mixed_dimensions_change_output_and_that_is_the_fix` for why the other
    // dimensions are deliberately NOT byte-identical.
    let table = stdout_of(&run(&["--last", "365d", "--group-by", "source"]));
    assert!(!table.contains('⚠'));
    assert!(!table.contains("budgets:"));
    let csv = stdout_of(&run(&["--last", "365d", "--group-by", "source", "--csv"]));
    assert!(!csv.contains("budget_state"));
    let json = json_of(&run(&["--last", "365d", "--group-by", "source", "--json"]));
    assert!(json.get("budgets").is_none());
    for row in json["rows"].as_array().unwrap() {
        assert!(row.get("budget_state").is_none());
    }
}

#[test]
fn mixed_dimensions_change_output_and_that_is_the_fix() {
    // The aggregator fix is a deliberate, no-budget behavior change: a group
    // spanning Sources is now `mixed`/cost-less instead of borrowing the first
    // Source's name and summed cost. That is why the snapshot guard above only
    // covers `--group-by source`. Assert the honest shape directly, so a future
    // "let's re-sum project costs" regression trips here rather than passing a
    // byte-identity check that only ever exercised the safety dimension.
    let json = json_of(&run(&["--last", "365d", "--group-by", "project", "--json"]));
    let shared = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "/home/hunter/repos/test")
        .expect("shared project row");
    // kilo + opencode both record cost; the group must NOT claim either one.
    assert_eq!(shared["cost"], serde_json::Value::Null);
    assert!(
        !shared["cost"].is_number(),
        "a mixed project must not report a summed cost: {shared}"
    );
    // `usage` names the same group `mixed`; both commands read one aggregator.
    let usage = Command::new(bin())
        .args(["usage", "--last", "365d", "--group-by", "project", "--json"])
        .args(fixture_paths())
        .output()
        .expect("failed to run llmhelper usage");
    let usage: serde_json::Value =
        serde_json::from_slice(&usage.stdout).expect("usage --json is not JSON");
    let usage_shared = usage["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["key"] == "/home/hunter/repos/test")
        .expect("shared project group");
    assert_eq!(
        usage_shared["source"], "mixed",
        "compare and usage must agree on the group's source: {usage_shared}"
    );
}

#[test]
fn json_carries_budget_state_only_with_budgets() {
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--json"]);
    args.extend(budget_args());
    let json = json_of(&run(&args));
    // The summary is present, and the over Source's row is `"over"`.
    assert_eq!(json["budgets"]["over"].as_u64().unwrap(), 1);
    let rows = json["rows"].as_array().unwrap();
    let opencode = rows.iter().find(|r| r["key"] == "opencode").unwrap();
    assert_eq!(opencode["budget_state"], "over");
    // A row whose Source has no budget omits the field entirely.
    let omp = rows.iter().find(|r| r["key"] == "omp").unwrap();
    assert!(omp.get("budget_state").is_none(), "omp was: {omp}");
    // A not-measured Source is present but rendered `not_measured`.
    let claude = rows.iter().find(|r| r["key"] == "claude").unwrap();
    assert_eq!(claude["budget_state"], "not_measured");
}

#[test]
fn csv_appends_budget_column_only_with_budgets() {
    // Without budgets: the original 15-column header, no budget column.
    let plain = stdout_of(&run(&["--last", "365d", "--group-by", "source", "--csv"]));
    let header = plain.lines().next().unwrap();
    assert_eq!(header.matches(',').count(), 14, "header was: {header}");
    assert!(!header.ends_with("budget_state"));

    // With budgets: a 16th column, `over` on the offending Source and empty on
    // the rest (never `under`).
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source", "--csv"]);
    args.extend(budget_args());
    let with = stdout_of(&run(&args));
    let mut lines = with.lines();
    let header = lines.next().unwrap();
    assert!(header.ends_with("budget_state"), "header was: {header}");
    let opencode = lines
        .find(|l| l.starts_with("1,opencode,"))
        .expect("opencode is rank 1 by tokens");
    assert!(opencode.ends_with(",over"), "row was: {opencode}");
}

#[test]
fn mixed_source_project_group_is_never_marked() {
    // `/home/hunter/repos/test` receives records from BOTH kilo and opencode,
    // and both report cost. It must aggregate as `mixed` (cost `—`) and be
    // excluded from a Source-scoped budget's marking, even though one of its
    // sources (kilo) is over its ceiling in aggregate. This is the regression
    // that a `Group.source` claiming the first source once hid.
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend([
        "--group-by",
        "project",
        "--budget",
        "kilo:1.0",
        "--budget-window",
        "365d",
    ]);
    let stdout = stdout_of(&run(&args));
    let shared = stdout
        .lines()
        .find(|l| l.contains("/home/hunter/repos/test"))
        .expect("shared project row");
    assert!(
        !shared.contains('⚠'),
        "a mixed-source project must not be marked: {shared}"
    );
    // The header still reports the over budget itself (kilo is over), so the
    // absence of a glyph is "no single row owns it", not "no budget".
    assert!(stdout.contains("budgets: 1 over"), "header was: {stdout}");
}

#[test]
fn exit_code_is_zero_with_an_over_budget() {
    // A budget is an annotation, not a gate (spec 0015).
    let mut args: Vec<&str> = WIDE.to_vec();
    args.extend(["--group-by", "source"]);
    args.extend(budget_args());
    let output = run(&args);
    assert!(output.status.success(), "over budget must not fail the run");
}

#[test]
fn unknown_budget_name_errors_before_loading() {
    let output = run(&["--last", "365d", "--budget-name", "nope"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown --budget-name 'nope'"),
        "stderr was: {stderr}"
    );
}

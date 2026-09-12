//! Integration tests for the empty-result diagnostic (`--explain`), driven
//! through the real CLI binary against the shared fixture tree. This is the
//! CLI seam from spec 0020.
//!
//! The subject is not the filter but its *explanation*: every read command must
//! be able to say why it produced nothing, and must stay silent about it when
//! the user did not ask and the result was not empty.

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
/// seven records across four sources.
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
/// test asserting a zero-loaded result can never observe the developer's real
/// data.
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

/// Run a read command with the fixture paths and the given extra arguments.
fn run(subcommand: &str, extra_args: &[&str]) -> Output {
    run_with(subcommand, &fixture_paths(), extra_args)
}

fn run_with(subcommand: &str, paths: &[String], extra_args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.arg(subcommand);
    cmd.args(paths);
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run llmhelper {subcommand}: {e}"))
}

/// The stderr of a run that must have succeeded, as a lossy string.
fn stderr_ok(output: &Output, context: &str) -> String {
    assert!(
        output.status.success(),
        "{context} exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The non-warning lines of stderr. Source load warnings (`warn: source ...`)
/// are orthogonal to the funnel and would otherwise pollute the assertions.
fn diagnostic_lines(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter(|l| !l.starts_with("warn:"))
        .map(|l| l.to_string())
        .collect()
}

/// Assert an over-narrow filter is explained, naming the flag the user typed,
/// and that the command still exits successfully — an empty result is a result.
fn assert_blames(subcommand: &str, extra_args: &[&str], expected: &[&str]) {
    let output = run(subcommand, extra_args);
    let stderr = stderr_ok(&output, subcommand);
    let diagnostic = diagnostic_lines(&stderr).join("\n");
    assert!(
        diagnostic.contains("no records matched"),
        "{subcommand} {extra_args:?} did not explain the empty result: {stderr}"
    );
    for needle in expected {
        assert!(
            diagnostic.contains(needle),
            "{subcommand} {extra_args:?} stderr missing {needle:?}: {stderr}"
        );
    }
    assert!(
        diagnostic.contains("filters: loaded"),
        "{subcommand} {extra_args:?} did not print the funnel: {stderr}"
    );
}

/// A project path no fixture record carries, so the project layer empties the
/// set on its own.
const NO_SUCH_PROJECT: &str = "/nonexistent-xyz";

#[test]
fn usage_explains_a_project_filter_that_excluded_everything() {
    assert_blames(
        "usage",
        &["--csv", "--project", NO_SUCH_PROJECT],
        &["excluded by --project", NO_SUCH_PROJECT],
    );
}

#[test]
fn sessions_explains_a_project_filter_that_excluded_everything() {
    assert_blames(
        "sessions",
        &["--csv", "--project", NO_SUCH_PROJECT],
        &["excluded by --project", NO_SUCH_PROJECT],
    );
}

#[test]
fn search_explains_a_project_filter_that_excluded_everything() {
    // `search` filters messages, not records, so its wording is its own: the
    // corpus is messages and the count it reports is messages.
    let output = run(
        "search",
        &["zzzznotfound", "--project", NO_SUCH_PROJECT, "--csv"],
    );
    let stderr = stderr_ok(&output, "search");
    assert!(
        stderr.contains("filters excluded every message"),
        "search did not explain the empty result: {stderr}"
    );
    assert!(stderr.contains("loaded"), "{stderr}");
}

#[test]
fn report_embeds_the_explanation_in_the_document() {
    // `report --output` has no terminal, so the file must explain its own
    // emptiness rather than leaving an empty document with no account of why.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.md");
    let output = run(
        "report",
        &[
            "--project",
            NO_SUCH_PROJECT,
            "--output",
            out.to_str().unwrap(),
        ],
    );
    stderr_ok(&output, "report");
    let markdown = std::fs::read_to_string(&out).unwrap();
    assert!(
        markdown.contains("## Filters"),
        "report document carries no filter section:\n{markdown}"
    );
    assert!(
        markdown.contains("no records matched"),
        "report document does not explain the emptiness:\n{markdown}"
    );
    assert!(
        markdown.contains(NO_SUCH_PROJECT),
        "report document omits the blamed value:\n{markdown}"
    );
}

#[test]
fn report_omits_the_filter_section_for_a_non_empty_result() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.md");
    let output = run("report", &["--output", out.to_str().unwrap()]);
    stderr_ok(&output, "report");
    let markdown = std::fs::read_to_string(&out).unwrap();
    assert!(
        !markdown.contains("## Filters"),
        "a non-empty report should carry no filter section:\n{markdown}"
    );
}

#[test]
fn diff_explains_the_window_that_excluded_everything() {
    let output = run(
        "diff",
        &[
            "--last",
            "1h",
            "--prev",
            "1h",
            "--project",
            NO_SUCH_PROJECT,
            "--csv",
        ],
    );
    let stderr = stderr_ok(&output, "diff");
    assert!(
        stderr.contains("prev window filters:"),
        "diff did not explain the previous window: {stderr}"
    );
    assert!(
        stderr.contains("curr window filters:"),
        "diff did not explain the current window: {stderr}"
    );
}

#[test]
fn watch_json_carries_the_same_explanation_as_a_usage_frame() {
    // In JSON mode the explanation travels in the payload, not on stderr: it
    // must survive a round trip through a consumer that only sees stdout.
    let json: serde_json::Value =
        serde_json::from_slice(&run("watch", &["--json", "--project", NO_SUCH_PROJECT]).stdout)
            .unwrap();
    let diagnostics = &json["diagnostics"];
    assert_eq!(diagnostics["loaded"], 7);
    assert_eq!(diagnostics["matched"], 0);
    assert_eq!(diagnostics["blamed"], "project");
}

#[test]
fn a_zero_loaded_run_blames_no_filter() {
    // No data on disk is not the user's filter's fault, however many predicates
    // are set. The absent paths guarantee the developer's real sources cannot
    // leak in and make this assertion vacuous.
    let dir = tempfile::tempdir().unwrap();
    let paths = absent_paths(&dir.path().join("nothing-here"));
    // The zero-loaded reason is reported by the CSV/table paths. JSON carries
    // it in the payload instead, so assert on stdout for the JSON subcommands.
    let csved = run_with("usage", &paths, &["--csv"]);
    let stderr = stderr_ok(&csved, "usage");
    let diagnostic = diagnostic_lines(&stderr).join("\n");
    assert!(
        diagnostic.contains("no records loaded from any source"),
        "usage blamed a filter for an empty disk: {stderr}"
    );
    assert!(
        !diagnostic.contains("excluded by"),
        "usage blamed a filter for an empty disk: {stderr}"
    );

    for subcommand in ["usage", "watch"] {
        let output = run_with(subcommand, &paths, &["--json"]);
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let diagnostics = &json["diagnostics"];
        assert_eq!(
            diagnostics["loaded"], 0,
            "{subcommand} did not report a zero-loaded run"
        );
        assert!(
            diagnostics["blamed"].is_null(),
            "{subcommand} blamed a filter for an empty disk: {diagnostics}"
        );
    }
}

#[test]
fn a_non_empty_result_without_explain_is_not_explained() {
    // The additive-only promise: a run that already worked must not gain noise.
    // `search` is excluded here because its TUI is the default and its
    // positional query needs a literal; it is asserted separately below.
    for (subcommand, extra) in [
        ("usage", vec!["--csv"]),
        ("sessions", vec!["--csv"]),
        ("watch", vec!["--json"]),
    ] {
        let output = run(subcommand, &extra);
        let stderr = stderr_ok(&output, subcommand);
        assert!(
            diagnostic_lines(&stderr).is_empty(),
            "{subcommand} explained a non-empty result without --explain: {stderr}"
        );
    }

    // A `search` whose query matches nothing still loads everything, so the
    // scope is non-empty; it must stay silent too.
    let output = run("search", &["a", "--csv"]);
    let stderr = stderr_ok(&output, "search");
    assert!(
        diagnostic_lines(&stderr).is_empty(),
        "search explained a non-empty result without --explain: {stderr}"
    );
}

#[test]
fn explain_prints_the_funnel_for_a_non_empty_result() {
    // With `--explain` the funnel is printed even when nothing was excluded:
    // the count of survivors is the answer to "where did they go?".
    let output = run("usage", &["--source", "claude", "--explain", "--csv"]);
    let stderr = stderr_ok(&output, "usage");
    assert!(
        stderr.contains("filters: loaded"),
        "usage --explain printed no funnel: {stderr}"
    );
    assert!(
        stderr.contains("source (\"claude\")"),
        "usage --explain did not name the source layer: {stderr}"
    );
    assert!(
        !stderr.contains("no records matched"),
        "a non-empty --explain run should not claim nothing matched: {stderr}"
    );
}

#[test]
fn explain_json_carries_a_well_formed_diagnostics_object() {
    for subcommand in ["usage", "watch"] {
        let json: serde_json::Value =
            serde_json::from_slice(&run(subcommand, &["--json", "--explain"]).stdout.clone())
                .unwrap_or_else(|e| panic!("{subcommand} --json --explain was not JSON: {e}"));
        let diagnostics = &json["diagnostics"];
        assert!(
            diagnostics.is_object(),
            "{subcommand} --json --explain has no diagnostics object: {json}"
        );
        assert_eq!(diagnostics["loaded"], 7);
        assert_eq!(diagnostics["matched"], 7);
        assert!(diagnostics["blamed"].is_null());
        let stages = diagnostics["stages"].as_array().unwrap();
        let layers: Vec<&str> = stages
            .iter()
            .map(|s| s["layer"].as_str().unwrap())
            .collect();
        assert_eq!(layers, ["window", "project", "model", "source"]);
    }
}

#[test]
fn explain_json_blames_the_layer_that_emptied_the_set() {
    let json: serde_json::Value =
        serde_json::from_slice(&run("usage", &["--json", "--project", NO_SUCH_PROJECT]).stdout)
            .unwrap();
    let diagnostics = &json["diagnostics"];
    assert_eq!(diagnostics["loaded"], 7);
    assert_eq!(diagnostics["matched"], 0);
    assert_eq!(diagnostics["blamed"], "project");
    let stages = diagnostics["stages"].as_array().unwrap();
    // The blamed stage carries the value the user typed; earlier stages are
    // passed through untouched, later ones are already empty.
    assert_eq!(stages[0]["remaining"], 7);
    assert_eq!(stages[1]["value"], NO_SUCH_PROJECT);
    assert_eq!(stages[1]["remaining"], 0);
}

#[test]
fn a_non_empty_json_result_is_byte_identical_to_the_pre_feature_shape() {
    // Without `--explain` and with results, the frame must not gain the
    // `diagnostics` key: downstream consumers parse this.
    let json: serde_json::Value =
        serde_json::from_slice(&run("usage", &["--json"]).stdout).unwrap();
    assert!(
        json.get("diagnostics").is_none(),
        "a non-empty run without --explain grew a diagnostics key: {json}"
    );
    // Parity for `sessions`, whose JSON is a bare array.
    let sessions: serde_json::Value =
        serde_json::from_slice(&run("sessions", &["--json"]).stdout).unwrap();
    assert!(
        sessions.is_array(),
        "sessions --json must stay a bare array: {sessions}"
    );
}

#[test]
fn a_zero_loaded_run_stays_exit_zero() {
    // An empty corpus is not an error, and the diagnostic must not turn it
    // into one.
    let dir = tempfile::tempdir().unwrap();
    let paths = absent_paths(&dir.path().join("nothing-here"));
    for subcommand in ["usage", "sessions", "watch"] {
        let output = run_with(subcommand, &paths, &["--json"]);
        assert!(
            output.status.success(),
            "{subcommand} exited {} on an empty corpus",
            output.status
        );
    }
    // `report` writes a document rather than a JSON frame.
    let out = dir.path().join("empty.md");
    let output = run_with("report", &paths, &["--output", out.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "report exited {} on an empty corpus",
        output.status
    );
}

#[test]
fn a_missing_session_detail_keeps_its_exit_code_and_message() {
    // The diagnostic is context for the miss, not a replacement for the error:
    // asking for a session that does not exist is still a failure.
    let output = run("sessions", &["--detail", "no-such-session-id"]);
    assert!(
        !output.status.success(),
        "a missing session detail must not succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session id not found"),
        "the id-specific error changed: {stderr}"
    );
}

/// The funnel's totals are the sum of the per-source loaded counts — the
/// arithmetic check the spec asks for against the machine's own sources.
#[test]
fn funnel_loaded_equals_the_sum_of_source_records() {
    let base = run("usage", &["--json", "--project", NO_SUCH_PROJECT]);
    assert!(base.status.success());
    let json: serde_json::Value = serde_json::from_slice(&base.stdout).unwrap();
    let per_source: u64 = json["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["records"].as_u64().unwrap_or(0))
        .sum();
    let loaded = json["diagnostics"]["loaded"].as_u64().unwrap();
    assert_eq!(
        loaded, per_source,
        "the funnel's loaded count disagrees with the per-source sum"
    );
    assert_eq!(loaded, 7, "the fixture tree loads seven records");
}

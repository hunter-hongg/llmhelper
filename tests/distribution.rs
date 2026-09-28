//! Spec 0028: the distribution surface — `--version`, the hidden `man`
//! subcommand, and the help text a new user actually reads.
//!
//! These assert observable behaviour (stdout bytes, exit codes, files on
//! disk), never clap's internals. The point of the man/completion generator
//! is that it is *derived* from the live `Cli` tree, so the tests assert that
//! real content exists rather than pinning roff bytes that would change on
//! every clap upgrade.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_llmhelper"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `llmhelper man` with the working directory set to a temp dir, so the
/// generated `target/dist` never touches the real checkout.
fn run_man_in(dir: &Path, extra: &[&str]) -> (bool, String, String) {
    let out = bin()
        .current_dir(dir)
        .arg("man")
        .args(extra)
        .output()
        .expect("run llmhelper man");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_flag_prints_the_package_version() {
    // Regression guard for a real distribution defect: without
    // `#[command(version)]` clap emits neither `--version` nor `-V`, and both
    // fail with "unexpected argument" — the most universal smoke test a
    // published CLI has to pass.
    for flag in ["--version", "-V"] {
        let out = bin().arg(flag).output().expect("run --version");
        assert!(
            out.status.success(),
            "`llmhelper {flag}` exited {:?}; stderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.starts_with("llmhelper 0.1.0"),
            "`{flag}` printed {:?}, expected it to start with the name and version",
            stdout
        );
    }
}

#[test]
fn the_man_subcommand_is_hidden_but_functional() {
    // Hidden from `--help` (it is a maintenance affordance, not a user
    // feature) yet still reachable, which is the whole point. The check is on
    // the Commands list specifically: a bare substring test would false-positive
    // on words like "Commands" and "man" inside a flag's help text.
    let help = bin().arg("--help").output().expect("run --help");
    let help_text = String::from_utf8_lossy(&help.stdout);
    let commands = help_text
        .split("Commands:")
        .nth(1)
        .expect("--help has no Commands section")
        .split("Options:")
        .next()
        .expect("--help has no Options section");
    assert!(
        !commands.contains("man "),
        "`man` should be hidden from the Commands list, but it appeared in:\n{commands}"
    );

    // …and it is still invocable.
    let out = bin().arg("man").arg("--stdout").output().expect("run man");
    assert!(
        out.status.success(),
        "hidden `man` is not invocable: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn help_lists_every_user_facing_subcommand() {
    let out = bin().arg("--help").output().expect("run --help");
    let text = String::from_utf8_lossy(&out.stdout);
    for sub in [
        "usage", "diff", "sessions", "report", "request", "search", "export", "watch", "trend",
        "compare",
    ] {
        assert!(
            text.contains(sub),
            "`{sub}` missing from --help; a new user cannot discover it:\n{text}"
        );
    }
}

#[test]
fn man_generates_a_page_per_subcommand_and_a_completion_per_shell() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ok, stdout, stderr) = run_man_in(tmp.path(), &[]);
    assert!(ok, "`man` failed: {stderr}");

    let dist = tmp.path().join("target").join("dist");
    for sub in [
        "usage", "diff", "sessions", "report", "request", "search", "export", "watch", "trend",
        "compare",
    ] {
        let page = dist.join(format!("{sub}.1"));
        let meta =
            std::fs::metadata(&page).unwrap_or_else(|e| panic!("missing man page {page:?}: {e}"));
        assert!(meta.len() > 0, "man page {page:?} is empty");
    }

    for shell in ["bash", "zsh", "fish", "powershell"] {
        let script = dist.join(format!("llmhelper.{shell}"));
        let meta = std::fs::metadata(&script)
            .unwrap_or_else(|e| panic!("missing completion {script:?}: {e}"));
        assert!(meta.len() > 0, "completion {script:?} is empty");
    }

    assert!(
        stdout.contains("wrote"),
        "expected one `wrote` line per artifact, got:\n{stdout}"
    );
}

#[test]
fn generated_man_page_documents_real_flags_and_text() {
    // Guards the "generated, not checked in" promise: a page that exists but
    // describes nothing would satisfy an existence-only test.
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ok, _, stderr) = run_man_in(tmp.path(), &[]);
    assert!(ok, "`man` failed: {stderr}");
    let page =
        std::fs::read_to_string(tmp.path().join("target/dist/usage.1")).expect("read usage.1");
    assert!(page.contains("llmhelper\\-usage"), "no NAME line:\n{page}");
    // A flag that exists in the CLI must appear in its man page.
    assert!(
        page.contains("claude"),
        "usage.1 omits --claude-dir:\n{page}"
    );
    assert!(
        page.contains("Show token usage"),
        "usage.1 lost its about text:\n{page}"
    );
}

#[test]
fn a_completion_script_offers_the_subcommands() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ok, _, stderr) = run_man_in(tmp.path(), &["--completions", "bash"]);
    assert!(ok, "`man --completions bash` failed: {stderr}");
    let script = std::fs::read_to_string(tmp.path().join("target/dist/llmhelper.bash"))
        .expect("read bash completion");
    for sub in ["usage", "search", "compare", "watch"] {
        assert!(
            script.contains(sub),
            "bash completion omits `{sub}`; Tab would not complete it"
        );
    }
}

#[test]
fn completions_can_be_streamed_to_stdout_for_a_single_shell() {
    // `--stdout` is what a package maintainer pipes into an install prefix, so
    // it must produce the script itself and not a list of paths.
    let out = bin()
        .arg("man")
        .arg("--completions")
        .arg("zsh")
        .arg("--stdout")
        .output()
        .expect("run man --stdout");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.trim().is_empty(),
        "--stdout produced nothing; a package maintainer needs the script on stdout"
    );
    assert!(
        !text.contains("wrote "),
        "--stdout printed the file list instead of the script:\n{text}"
    );
}

#[test]
fn the_man_subcommand_reads_no_usage_data() {
    // It must not require config or sources: it is a documentation tool, and
    // a user on a machine with no agent data still needs the man page.
    let out = bin()
        .arg("man")
        .arg("--stdout")
        .arg("--completions")
        .arg("fish")
        .output()
        .expect("run man");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("no records loaded"),
        "`man` consulted the usage sources:\n{stderr}"
    );
    assert!(
        !stderr.contains("warn:"),
        "`man` emitted a warning, meaning it touched real state:\n{stderr}"
    );
}

#[test]
fn the_repository_license_and_manifest_are_publishable() {
    // Not a CLI behaviour, but it is the same contract: a crate without a
    // license is rejected by crates.io, so the metadata is load-bearing.
    let manifest =
        std::fs::read_to_string(repo_root().join("Cargo.toml")).expect("read Cargo.toml");
    assert!(
        manifest.contains("license = \"MIT\""),
        "Cargo.toml declares no license; crates.io will reject the crate"
    );
    assert!(
        manifest.contains("readme = \"README.md\""),
        "Cargo.toml declares no readme; the crates.io page would be empty"
    );
    let license = repo_root().join("LICENSE");
    assert!(
        std::fs::metadata(&license).is_ok(),
        "LICENSE file is missing while Cargo.toml claims MIT"
    );
    let keywords = manifest
        .lines()
        .find(|l| l.starts_with("keywords ="))
        .expect("keywords line");
    let count = keywords.matches('"').count() / 2;
    assert!(
        count <= 5,
        "crates.io allows at most 5 keywords, found {count} in: {keywords}"
    );
}

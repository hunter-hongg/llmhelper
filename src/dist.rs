//! Man page and shell completion generation (spec 0028).
//!
//! Both artifacts are rendered from the *live* [`Cli`](crate::cli::Cli) tree
//! at run time, so neither can drift from the flags the binary actually
//! accepts. A checked-in man page is worse than none, because a user trusts
//! it: the failure mode is a documented flag that errors.
//!
//! The generator ships **in** the binary rather than living in `build.rs`
//! for two reasons. `build.rs` cannot `include!` `cli.rs` — the CLI module
//! depends on the rest of the crate (`crate::filter`, `crate::config`, …),
//! so a build script would have to depend on the crate it builds. And the
//! user who wants a man page is the user who installed the binary, so making
//! them install a separate tool to generate one is a pointless extra step.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Command, CommandFactory, ValueEnum};

use crate::cli::{Cli, CompletionShellArg, ManArgs};

/// The shells we generate for, in a fixed order so output is reproducible.
const SHELLS: [CompletionShellArg; 4] = [
    CompletionShellArg::Bash,
    CompletionShellArg::Zsh,
    CompletionShellArg::Fish,
    CompletionShellArg::PowerShell,
];

fn shell_extension(shell: clap_complete::Shell) -> String {
    // `Shell` implements `ValueEnum`, so its canonical name is the same string
    // `--completions` accepts. Deriving the extension from that one source
    // keeps the file name and the flag's accepted value from disagreeing.
    shell
        .to_possible_value()
        .map(|v| v.get_name().to_string())
        .unwrap_or_else(|| "sh".to_string())
}

/// One roff man page for `cmd`, written to `dir`.
///
/// `clap_mangen` streams its output, so the target must be a real file: a pipe
/// would deadlock on the first write.
fn render_man_page(cmd: &mut Command, dir: &Path) -> anyhow::Result<String> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.1", cmd.get_name()));
    let mut file = std::fs::File::create(&path)?;
    clap_mangen::Man::new(cmd.clone()).render(&mut file)?;
    Ok(path.display().to_string())
}

fn completion_script(shell: clap_complete::Shell, cmd: &mut Command) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(shell, cmd, "llmhelper", &mut buf);
    buf
}

/// Run the `man` subcommand. Prints one `wrote <path>` line per artifact.
pub fn run(args: &ManArgs) -> anyhow::Result<()> {
    // `build()` mutates in place and returns `()`: it materializes defaults
    // and propagates global flags, so what follows reflects what parsing will
    // actually accept.
    let mut cmd = Cli::command();
    cmd.build();

    assert!(
        cmd.get_subcommands().next().is_some(),
        "Cli exposes no subcommands; generated documentation would be empty"
    );

    let shells: Vec<CompletionShellArg> = match args.completions {
        Some(shell) => vec![shell],
        None => SHELLS.to_vec(),
    };

    let mut written: Vec<String> = Vec::new();

    // `--stdout` streams *only* the requested completion script: a package
    // maintainer pipes it straight into a prefix, so anything else on stdout
    // would corrupt the file they are installing. Man pages are roff and
    // `clap_mangen` needs a seekable sink, so they are never streamed.
    if args.stdout {
        for shell in shells {
            let script = completion_script(shell.into(), &mut cmd);
            std::io::stdout().write_all(&script)?;
        }
        std::io::stdout().flush()?;
        return Ok(());
    }

    let dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("target").join("dist"));

    // The hidden `man` subcommand documents itself like any other: including
    // it costs nothing and keeps `man.1` truthful if it ever becomes visible.
    for sub in cmd.get_subcommands_mut() {
        let path = render_man_page(sub, &dir)?;
        written.push(path);
    }

    std::fs::create_dir_all(&dir)?;
    for shell in shells {
        let path = dir.join(format!("llmhelper.{}", shell_extension(shell.into())));
        let mut file = std::fs::File::create(&path)?;
        file.write_all(&completion_script(shell.into(), &mut cmd))?;
        written.push(path.display().to_string());
    }

    for path in written {
        println!("wrote {path}");
    }
    Ok(())
}

/// Does `path` exist, and is it non-empty? Used by the tests that assert the
/// generator produced real content rather than a zero-byte file.
#[allow(dead_code)]
pub(crate) fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

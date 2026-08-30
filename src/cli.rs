use std::time::Duration;

use clap::Parser;
use chrono::{DateTime, Utc};

/// Main CLI entry point.
#[derive(Parser, Debug)]
#[command(name = "llmhelper", about = "Agent usage introspection")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Show token usage and activity across Agent/LLM sources.
    Usage(UsageArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct UsageArgs {
    /// Claude Code projects directory (defaults to ~/.claude/projects).
    #[arg(long = "claude-dir")]
    pub claude_dir: Option<std::path::PathBuf>,

    /// OpenCode database path(s). Can be specified multiple times.
    #[arg(long = "opencode-db")]
    pub opencode_db: Option<Vec<std::path::PathBuf>>,

    /// Only include sessions started at or after this RFC 3339 timestamp.
    #[arg(long = "since")]
    pub since: Option<DateTime<Utc>>,

    /// Only include sessions from the last N days/hours (e.g. "7d", "4h").
    /// Mutually exclusive with --since.
    #[arg(long = "last")]
    pub last: Option<String>,

    /// Filter by project path substring.
    #[arg(long = "project")]
    pub project: Option<String>,

    /// Filter by model substring (case-insensitive).
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Filter by source name: "claude" or "opencode".
    #[arg(long = "source")]
    pub source: Option<String>,

    /// Output as JSON instead of the interactive TUI.
    #[arg(long = "json")]
    pub json: bool,

    /// Output as CSV instead of the interactive TUI.
    #[arg(long = "csv")]
    pub csv: bool,
}

impl UsageArgs {
    /// Parse --last duration string into a Duration.
    pub fn parse_last(&self) -> Option<Duration> {
        let s = self.last.as_deref()?;
        if s.ends_with('d') {
            let n: u64 = s[..s.len() - 1].parse().ok()?;
            Some(Duration::from_secs(n * 24 * 3600))
        } else if s.ends_with('h') {
            let n: u64 = s[..s.len() - 1].parse().ok()?;
            Some(Duration::from_secs(n * 3600))
        } else if s.ends_with('m') {
            let n: u64 = s[..s.len() - 1].parse().ok()?;
            Some(Duration::from_secs(n * 60))
        } else {
            None
        }
    }

    /// Validate mutually-exclusive flag combinations.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.json && self.csv {
            anyhow::bail!("--json and --csv are mutually exclusive");
        }
        if self.since.is_some() && self.last.is_some() {
            anyhow::bail!("--since and --last are mutually exclusive");
        }
        Ok(())
    }
}

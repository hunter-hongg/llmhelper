use std::time::Duration;

use clap::{Parser, ValueEnum};
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
    /// Compare token usage between two time windows.
    Diff(DiffArgs),
    /// List individual Agent Sessions.
    Sessions(SessionsArgs),
}

#[derive(Clone, Debug, Default, ValueEnum, PartialEq, Eq)]
pub enum GroupByArg {
    #[default]
    Source,
    Project,
    Model,
}

impl std::fmt::Display for GroupByArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source => write!(f, "source"),
            Self::Project => write!(f, "project"),
            Self::Model => write!(f, "model"),
        }
    }
}

impl From<GroupByArg> for crate::domain::group::GroupBy {
    fn from(v: GroupByArg) -> Self {
        match v {
            GroupByArg::Source => Self::Source,
            GroupByArg::Project => Self::Project,
            GroupByArg::Model => Self::Model,
        }
    }
}

#[derive(Clone, Debug, Default, ValueEnum, PartialEq, Eq)]
pub enum SourceArg {
    #[default]
    Claude,
    Opencode,
    Omp,
    Kilo,
}

impl std::fmt::Display for SourceArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Opencode => write!(f, "opencode"),
            Self::Omp => write!(f, "omp"),
            Self::Kilo => write!(f, "kilo"),
        }
    }
}

#[derive(Parser, Debug, Clone, Default)]
pub struct UsageArgs {
    /// Claude Code projects directory (defaults to ~/.claude/projects).
    #[arg(long = "claude-dir")]
    pub claude_dir: Option<std::path::PathBuf>,

    /// OpenCode database path(s). Can be specified multiple times.
    #[arg(long = "opencode-db")]
    pub opencode_db: Option<Vec<std::path::PathBuf>>,

    /// OMP sessions directory (defaults to ~/.omp/agent/sessions).
    #[arg(long = "omp-dir")]
    pub omp_dir: Option<std::path::PathBuf>,

    /// Kilo Code database path(s). Can be specified multiple times.
    #[arg(long = "kilo-db")]
    pub kilo_db: Option<Vec<std::path::PathBuf>>,

    /// Only include sessions started at or after this RFC 3339 timestamp.
    #[arg(long = "since")]
    pub since: Option<DateTime<Utc>>,

    /// Only include sessions from the last N days/hours (e.g. "7d", "4h").
    /// Mutually exclusive with --since. Errors on unparseable input.
    #[arg(long = "last")]
    pub last: Option<String>,

    /// Filter by project path substring.
    #[arg(long = "project")]
    pub project: Option<String>,

    /// Filter by model substring (case-insensitive).
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Filter by source name.
    #[arg(long = "source")]
    pub source: Option<SourceArg>,

    /// Group output by this dimension: source, project, or model.
    #[arg(long = "group-by", default_value_t)]
    pub group_by: GroupByArg,

    /// Output as JSON instead of the interactive TUI.
    #[arg(long = "json")]
    pub json: bool,

    /// Output as CSV instead of the interactive TUI.
    #[arg(long = "csv")]
    pub csv: bool,
}

impl UsageArgs {
    /// Parse --last duration string into a Duration. Errors on bad input.
    pub fn parse_last(&self) -> anyhow::Result<Option<Duration>> {
        let s = match &self.last {
            Some(s) => s,
            None => return Ok(None),
        };
        parse_duration(s).map_err(|e| anyhow::anyhow!("invalid --last {}", e)).map(Some)
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

/// Parse a duration string in `Nd`, `Nh`, `Nm`, or `Ns` form (e.g. `7d`, `24h`, `30m`, `86400s`).
pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    if s.ends_with('d') {
        let n: u64 = s[..s.len() - 1]
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid duration '{}': {}", s, e))?;
        return Ok(Duration::from_secs(n * 24 * 3600));
    }
    if s.ends_with('h') {
        let n: u64 = s[..s.len() - 1]
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid duration '{}': {}", s, e))?;
        return Ok(Duration::from_secs(n * 3600));
    }
    if s.ends_with('m') {
        let n: u64 = s[..s.len() - 1]
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid duration '{}': {}", s, e))?;
        return Ok(Duration::from_secs(n * 60));
    }
    if s.ends_with('s') {
        let n: u64 = s[..s.len() - 1]
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid duration '{}': {}", s, e))?;
        return Ok(Duration::from_secs(n));
    }
    Err(anyhow::anyhow!(
        "invalid duration '{}': expected N d/h/m/s (e.g. 7d, 4h, 30m, 86400s)",
        s
    ))
}

#[derive(Parser, Debug, Clone)]
pub struct DiffArgs {
    /// Current window length, e.g. "7d" or "4h". Required.
    #[arg(long = "last")]
    pub last: Option<String>,

    /// Previous window length, e.g. "7d" or "4h". Required.
    #[arg(long = "prev")]
    pub prev: Option<String>,

    /// Claude Code projects directory (defaults to ~/.claude/projects).
    #[arg(long = "claude-dir")]
    pub claude_dir: Option<std::path::PathBuf>,

    /// OpenCode database path(s). Can be specified multiple times.
    #[arg(long = "opencode-db")]
    pub opencode_db: Option<Vec<std::path::PathBuf>>,

    /// OMP sessions directory (defaults to ~/.omp/agent/sessions).
    #[arg(long = "omp-dir")]
    pub omp_dir: Option<std::path::PathBuf>,

    /// Kilo Code database path(s). Can be specified multiple times.
    #[arg(long = "kilo-db")]
    pub kilo_db: Option<Vec<std::path::PathBuf>>,

    /// Filter by project path substring.
    #[arg(long = "project")]
    pub project: Option<String>,

    /// Filter by model substring (case-insensitive).
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Filter by source name.
    #[arg(long = "source")]
    pub source: Option<SourceArg>,

    /// Group output by this dimension: source, project, or model.
    #[arg(long = "group-by", default_value_t)]
    pub group_by: GroupByArg,

    /// Output as JSON instead of the terminal table.
    #[arg(long = "json")]
    pub json: bool,

    /// Output as CSV instead of the terminal table.
    #[arg(long = "csv")]
    pub csv: bool,
}

impl DiffArgs {
    /// Validate that --last and --prev are present and --json/--csv are mutually exclusive.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.json && self.csv {
            anyhow::bail!("--json and --csv are mutually exclusive");
        }
        Ok(())
    }

    /// Parse both --last and --prev into Durations. Errors on missing or malformed input.
    pub fn parse_windows(&self) -> anyhow::Result<(Duration, Duration)> {
        let last = match &self.last {
            Some(s) => parse_duration(s).map_err(|e| anyhow::anyhow!("invalid --last {}", e))?,
            None => anyhow::bail!("--last is required"),
        };
        let prev = match &self.prev {
            Some(s) => parse_duration(s).map_err(|e| anyhow::anyhow!("invalid --prev {}", e))?,
            None => anyhow::bail!("--prev is required"),
        };
        Ok((last, prev))
    }
}

#[derive(Parser, Debug, Clone)]
pub struct SessionsArgs {
    /// Claude Code projects directory (defaults to ~/.claude/projects).
    #[arg(long = "claude-dir")]
    pub claude_dir: Option<std::path::PathBuf>,

    /// OpenCode database path(s). Can be specified multiple times.
    #[arg(long = "opencode-db")]
    pub opencode_db: Option<Vec<std::path::PathBuf>>,

    /// OMP sessions directory (defaults to ~/.omp/agent/sessions).
    #[arg(long = "omp-dir")]
    pub omp_dir: Option<std::path::PathBuf>,

    /// Kilo Code database path(s). Can be specified multiple times.
    #[arg(long = "kilo-db")]
    pub kilo_db: Option<Vec<std::path::PathBuf>>,

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

    /// Filter by source name.
    #[arg(long = "source")]
    pub source: Option<SourceArg>,

    /// Limit number of sessions listed.
    #[arg(long = "limit")]
    pub limit: Option<usize>,

    /// Offset for pagination.
    #[arg(long = "offset")]
    pub offset: Option<usize>,

    /// Show detail for a specific session id.
    #[arg(long = "detail")]
    pub detail: Option<String>,

    /// Output as JSON instead of the terminal table.
    #[arg(long = "json")]
    pub json: bool,

    /// Output as CSV instead of the terminal table.
    #[arg(long = "csv")]
    pub csv: bool,
}

impl SessionsArgs {
    pub fn parse_last(&self) -> anyhow::Result<Option<Duration>> {
        let s = match &self.last {
            Some(s) => s,
            None => return Ok(None),
        };
        parse_duration(s).map_err(|e| anyhow::anyhow!("invalid --last {}", e)).map(Some)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.json && self.csv {
            anyhow::bail!("--json and --csv are mutually exclusive");
        }
        if self.since.is_some() && self.last.is_some() {
            anyhow::bail!("--since and --last are mutually exclusive");
        }
        if self.detail.is_some() && (self.limit.is_some() || self.offset.is_some()) {
            anyhow::bail!("--detail cannot be combined with --limit/--offset");
        }
        Ok(())
    }
}

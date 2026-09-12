use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Parser, ValueEnum};

/// The time/scope predicates every read-only command shares.
///
/// Every subcommand that loads records accepts the same four scoping flags
/// (`--since`/`--last`, `--project`, `--model`, `--source`), plus an optional
/// per-Source tolerance flag. Implementing this trait lets `main.rs` build a
/// [`Filter`] from any of them with one function, so adding a new shared
/// predicate means implementing one accessor rather than editing five
/// near-identical builders.
///
/// [`Filter`]: crate::filter::Filter
pub trait FilterArgs {
    fn since(&self) -> Option<DateTime<Utc>>;
    fn last(&self) -> Option<&str>;
    fn project(&self) -> Option<&str>;
    fn model(&self) -> Option<&str>;
    fn source(&self) -> Option<&SourceArg>;

    /// Whether a Source that fails to load should contribute zero records
    /// instead of failing the command. Only `search` exposes this as a flag;
    /// everything else treats a load error as fatal to the whole run.
    fn fail_open(&self) -> bool {
        false
    }

    /// Parse `--last` into a `Duration`. Errors name the flag on bad input.
    fn parse_last(&self) -> anyhow::Result<Option<Duration>> {
        let Some(s) = self.last() else {
            return Ok(None);
        };
        crate::cli::parse_duration(s)
            .map_err(|e| anyhow::anyhow!("invalid --last {}", e))
            .map(Some)
    }
}

/// The four source-path override flags shared by every command that reads data.
///
/// Implementing this trait lets `main.rs` merge CLI overrides onto a loaded
/// [`Config`] with one function instead of a copy per command.
///
/// [`Config`]: crate::config::Config
pub trait SourcePathArgs {
    fn claude_dir(&self) -> Option<&PathBuf>;
    fn opencode_db(&self) -> Option<&Vec<PathBuf>>;
    fn omp_dir(&self) -> Option<&PathBuf>;
    fn kilo_db(&self) -> Option<&Vec<PathBuf>>;
}

/// Apply CLI source-path overrides on top of a loaded config, with the CLI
/// winning. Every non-overridden key is passed through untouched.
pub fn merge_source_paths<A: SourcePathArgs>(
    mut config: crate::config::Config,
    args: &A,
) -> crate::config::Config {
    if let Some(v) = args.claude_dir() {
        config.claude_dir = Some(v.clone());
    }
    if let Some(v) = args.opencode_db() {
        config.opencode_dbs = Some(v.clone());
    }
    if let Some(v) = args.omp_dir() {
        config.omp_dir = Some(v.clone());
    }
    if let Some(v) = args.kilo_db() {
        config.kilo_dbs = Some(v.clone());
    }
    config
}

/// Main CLI entry point.
#[derive(Parser, Debug)]
#[command(name = "llmhelper", about = "Agent usage introspection")]
pub struct Cli {
    /// Path to a TOML config file. Defaults to
    /// ~/.config/llmhelper/config.toml.
    #[arg(long = "config", global = true)]
    pub config: Option<std::path::PathBuf>,

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
    /// Render a shareable Markdown summary of filtered Usage.
    Report(ReportArgs),
    /// Send request to OpenAI compatible endpoint.
    Request(RequestArgs),
    /// Search message text across Agent/LLM transcripts.
    Search(SearchArgs),
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

impl FilterArgs for UsageArgs {
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    fn source(&self) -> Option<&SourceArg> {
        self.source.as_ref()
    }
}

impl SourcePathArgs for UsageArgs {
    fn claude_dir(&self) -> Option<&PathBuf> {
        self.claude_dir.as_ref()
    }
    fn opencode_db(&self) -> Option<&Vec<PathBuf>> {
        self.opencode_db.as_ref()
    }
    fn omp_dir(&self) -> Option<&PathBuf> {
        self.omp_dir.as_ref()
    }
    fn kilo_db(&self) -> Option<&Vec<PathBuf>> {
        self.kilo_db.as_ref()
    }
}

/// Parse a duration string in `Nd`, `Nh`, `Nm`, or `Ns` form (e.g. `7d`, `24h`, `30m`, `86400s`).
fn parse_numeric_prefix(s: &str, suffix: char) -> anyhow::Result<u64> {
    let Some(rest) = s.strip_suffix(suffix) else {
        anyhow::bail!("invalid duration '{}': missing suffix {}", s, suffix);
    };
    rest.parse()
        .map_err(|e| anyhow::anyhow!("invalid duration '{}': {}", s, e))
}

pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    if s.ends_with('d') {
        let n = parse_numeric_prefix(s, 'd')?;
        return Ok(Duration::from_secs(n * 24 * 3600));
    }
    if s.ends_with('h') {
        let n = parse_numeric_prefix(s, 'h')?;
        return Ok(Duration::from_secs(n * 3600));
    }
    if s.ends_with('m') {
        let n = parse_numeric_prefix(s, 'm')?;
        return Ok(Duration::from_secs(n * 60));
    }
    if s.ends_with('s') {
        let n = parse_numeric_prefix(s, 's')?;
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

impl SourcePathArgs for DiffArgs {
    fn claude_dir(&self) -> Option<&PathBuf> {
        self.claude_dir.as_ref()
    }
    fn opencode_db(&self) -> Option<&Vec<PathBuf>> {
        self.opencode_db.as_ref()
    }
    fn omp_dir(&self) -> Option<&PathBuf> {
        self.omp_dir.as_ref()
    }
    fn kilo_db(&self) -> Option<&Vec<PathBuf>> {
        self.kilo_db.as_ref()
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

impl FilterArgs for SessionsArgs {
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    fn source(&self) -> Option<&SourceArg> {
        self.source.as_ref()
    }
}

impl SourcePathArgs for SessionsArgs {
    fn claude_dir(&self) -> Option<&PathBuf> {
        self.claude_dir.as_ref()
    }
    fn opencode_db(&self) -> Option<&Vec<PathBuf>> {
        self.opencode_db.as_ref()
    }
    fn omp_dir(&self) -> Option<&PathBuf> {
        self.omp_dir.as_ref()
    }
    fn kilo_db(&self) -> Option<&Vec<PathBuf>> {
        self.kilo_db.as_ref()
    }
}

#[derive(Parser, Debug, Clone)]
pub struct ReportArgs {
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

    /// Group output by this dimension: source, project, or model.
    #[arg(long = "group-by", default_value_t)]
    pub group_by: GroupByArg,

    /// Show at most the n largest groups by input tokens (must be >= 1).
    #[arg(long = "top")]
    pub top: Option<usize>,

    /// Custom Markdown document title. Defaults to "llmhelper report".
    #[arg(long = "title")]
    pub title: Option<String>,

    /// Write the rendered report to this file instead of stdout.
    #[arg(long = "output")]
    pub output: Option<std::path::PathBuf>,
}

impl ReportArgs {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.since.is_some() && self.last.is_some() {
            anyhow::bail!("--since and --last are mutually exclusive");
        }
        if let Some(top) = self.top {
            if top == 0 {
                anyhow::bail!("--top must be at least 1");
            }
        }
        Ok(())
    }
}

impl FilterArgs for ReportArgs {
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    fn source(&self) -> Option<&SourceArg> {
        self.source.as_ref()
    }
}

impl SourcePathArgs for ReportArgs {
    fn claude_dir(&self) -> Option<&PathBuf> {
        self.claude_dir.as_ref()
    }
    fn opencode_db(&self) -> Option<&Vec<PathBuf>> {
        self.opencode_db.as_ref()
    }
    fn omp_dir(&self) -> Option<&PathBuf> {
        self.omp_dir.as_ref()
    }
    fn kilo_db(&self) -> Option<&Vec<PathBuf>> {
        self.kilo_db.as_ref()
    }
}

#[derive(Parser, Debug, Clone, Default)]
pub struct RequestArgs {
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
    #[arg(long = "api-key")]
    pub api_key: Option<String>,
    #[arg(long = "model")]
    pub model: Option<String>,
    #[arg(long = "messages")]
    pub messages: Option<std::path::PathBuf>,
    #[arg(long = "prompt")]
    pub prompt: Option<String>,
    /// Path to a JSON file of OpenAI tool definitions (an array). The array is
    /// embedded verbatim in the request `tools` field.
    #[arg(long = "tools")]
    pub tools: Option<std::path::PathBuf>,
    /// Response field carrying reasoning text, as a dotted path (e.g.
    /// `choices.0.message.reasoning`, `delta.reasoning_content`). Repeatable:
    /// every named field is consulted and the non-empty ones are joined.
    /// Omit to disable reasoning capture entirely.
    #[arg(long = "reasoning-field")]
    pub reasoning_field: Vec<String>,
    /// Path to a JSON file holding a reasoning configuration object (effort,
    /// budget, ...). The object is embedded verbatim as the payload's
    /// `reasoning` key; this command does not interpret it.
    #[arg(long = "reasoning")]
    pub reasoning: Option<std::path::PathBuf>,
    /// Show the reasoning channel expanded in the TUI. Composes with
    /// --interactive; `t` cycles the view at runtime.
    #[arg(long = "thinking")]
    pub thinking: bool,
    #[arg(long = "json")]
    pub json: bool,
    #[arg(long = "text")]
    pub text: bool,
    #[arg(long = "stream")]
    pub stream: bool,
    /// Open an interactive TUI: after each response, type a follow-up and
    /// press Enter to send it as the next user turn. Mutually exclusive with
    /// --json and --text.
    #[arg(long = "interactive")]
    pub interactive: bool,
    /// Append the request and response to a per-day log file under
    /// ~/.config/llmhelper/logs/. Off by default; the API key is never
    /// written.
    #[arg(long = "log")]
    pub log: bool,
    /// After a successful TUI view, copy the assistant content to the
    /// terminal clipboard via the OSC 52 sequence. Requires a terminal that
    /// supports OSC 52. Mutually exclusive with --json and --text.
    #[arg(long = "copy")]
    pub copy: bool,
    #[arg(long = "temperature")]
    pub temperature: Option<f32>,
    #[arg(long = "top-p")]
    pub top_p: Option<f32>,
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,
    #[arg(long = "stop")]
    pub stop: Vec<String>,
}

impl RequestArgs {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.json && self.text {
            anyhow::bail!("--json and --text are mutually exclusive");
        }
        if self.messages.is_some() && self.prompt.is_some() {
            anyhow::bail!("--messages and --prompt are mutually exclusive");
        }
        if self.interactive && (self.json || self.text) {
            anyhow::bail!("--interactive is mutually exclusive with --json and --text");
        }
        if self.copy && (self.json || self.text) {
            anyhow::bail!("--copy is mutually exclusive with --json and --text");
        }
        Ok(())
    }
}

#[derive(Parser, Debug, Clone)]
pub struct SearchArgs {
    /// Text to search for across message content.
    pub query: String,

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

    /// Only include messages at or after this RFC 3339 timestamp.
    #[arg(long = "since")]
    pub since: Option<DateTime<Utc>>,

    /// Only include messages from the last N days/hours (e.g. "7d", "4h").
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

    /// Filter by message role, e.g. user, assistant, or thinking.
    #[arg(long = "role")]
    pub role: Option<String>,

    /// Match the query with case sensitivity. Case-insensitive by default.
    #[arg(long = "case-sensitive")]
    pub case_sensitive: bool,

    /// Allow messages without a recorded timestamp to pass through --since
    /// and --last time predicates. Default (fail-closed) excludes them.
    #[arg(long = "fail-open")]
    pub fail_open: bool,

    /// Characters of context on either side of a match in the snippet.
    #[arg(long = "context", default_value_t = crate::search::DEFAULT_CONTEXT)]
    pub context: usize,

    /// Show at most this many hits (must be >= 1).
    #[arg(long = "limit", default_value_t = crate::search::DEFAULT_LIMIT)]
    pub limit: usize,

    /// Output as JSON instead of the interactive TUI.
    #[arg(long = "json")]
    pub json: bool,

    /// Output as CSV instead of the interactive TUI.
    #[arg(long = "csv")]
    pub csv: bool,

    /// Output a plain hit list instead of the interactive TUI.
    #[arg(long = "text")]
    pub text: bool,
}

impl SearchArgs {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.query.trim().is_empty() {
            anyhow::bail!("query must not be empty");
        }
        if self.json && self.csv {
            anyhow::bail!("--json and --csv are mutually exclusive");
        }
        if self.json && self.text {
            anyhow::bail!("--json and --text are mutually exclusive");
        }
        if self.csv && self.text {
            anyhow::bail!("--csv and --text are mutually exclusive");
        }
        if self.since.is_some() && self.last.is_some() {
            anyhow::bail!("--since and --last are mutually exclusive");
        }
        if self.limit == 0 {
            anyhow::bail!("--limit must be at least 1");
        }
        Ok(())
    }
}

impl FilterArgs for SearchArgs {
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    fn source(&self) -> Option<&SourceArg> {
        self.source.as_ref()
    }
    fn fail_open(&self) -> bool {
        self.fail_open
    }
}

impl SourcePathArgs for SearchArgs {
    fn claude_dir(&self) -> Option<&PathBuf> {
        self.claude_dir.as_ref()
    }
    fn opencode_db(&self) -> Option<&Vec<PathBuf>> {
        self.opencode_db.as_ref()
    }
    fn omp_dir(&self) -> Option<&PathBuf> {
        self.omp_dir.as_ref()
    }
    fn kilo_db(&self) -> Option<&Vec<PathBuf>> {
        self.kilo_db.as_ref()
    }
}

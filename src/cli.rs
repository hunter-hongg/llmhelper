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

/// The single-window scoping flags shared by the commands that take one time
/// window (`usage`, `report`).
///
/// Both accept `--last`, optionally aligned to a calendar bucket with
/// `--calendar`. Implementing this trait makes both commands derive their
/// window the same way and reject the same conflicting combinations, so the two
/// cannot drift.
pub trait WindowArgs {
    fn last(&self) -> Option<&str>;
    fn since(&self) -> Option<DateTime<Utc>>;
    fn calendar(&self) -> bool;

    /// Validate the flag combination: `--since` and `--last` are mutually
    /// exclusive, and `--calendar` names a lower bound of its own so it cannot
    /// be combined with `--since` and must have a `--last` to anchor.
    fn validate_window(&self) -> anyhow::Result<()> {
        if self.since().is_some() && self.last().is_some() {
            anyhow::bail!("--since and --last are mutually exclusive");
        }
        if self.calendar() {
            if self.since().is_some() {
                anyhow::bail!("--calendar and --since are mutually exclusive");
            }
            if self.last().is_none() {
                anyhow::bail!("--calendar requires --last");
            }
        }
        Ok(())
    }

    /// The window the command scopes its records to, resolved against `now`.
    ///
    /// Rolling mode parses `--last` as a duration (`7d`, `4h`). Calendar mode
    /// accepts only the keywords shared with `budget` (`1d`/`1w`/`1mo`): a
    /// rolling duration has no calendar meaning and is a loud error rather than
    /// a silently misaligned window.
    ///
    /// Returns `None` when no window is requested (no `--last`).
    fn window_mode(&self) -> anyhow::Result<Option<crate::domain::window::WindowMode>> {
        let Some(last) = self.last() else {
            return Ok(None);
        };
        if self.calendar() {
            let days = crate::domain::window::calendar_days(last).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid --last for --calendar: '{}' (expected calendar window: 1d, 1w, 1mo)",
                    last
                )
            })?;
            Ok(Some(crate::domain::window::WindowMode::Calendar { days }))
        } else {
            let d = parse_duration(last).map_err(|e| anyhow::anyhow!("invalid --last {}", e))?;
            let d = chrono::Duration::from_std(d)?;
            Ok(Some(crate::domain::window::WindowMode::Rolling { last: d }))
        }
    }
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
    /// Export normalized Session records for downstream tools.
    Export(ExportArgs),
    /// Continuously monitor usage, cost and budget status.
    Watch(WatchArgs),
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

/// The three budget flags shared by the commands that display budgets
/// (`usage` and `report`). Kept off [`FilterArgs`] deliberately: a budget is
/// not a record predicate, it is an annotation applied to already-filtered
/// records, so the two concerns stay separate.
pub trait BudgetArgs {
    /// One-off budgets spelled `source:amount`, repeatable.
    fn budget(&self) -> &[String];
    /// Window applied to `--budget` one-offs. `None` means the default `1d`.
    fn budget_window(&self) -> Option<&str>;
    /// Restrict evaluation to these configured budget names, repeatable.
    fn budget_name(&self) -> &[String];

    /// Resolve the effective budget list: the named configured budgets (or all
    /// of them when no name is given) plus any `--budget` one-offs. Every
    /// returned budget is validated here so a bad flag fails before loading.
    fn resolve_budgets(
        &self,
        configured: &[crate::budget::Budget],
    ) -> anyhow::Result<Vec<crate::budget::Budget>> {
        let mut out: Vec<crate::budget::Budget> = Vec::new();

        let names = self.budget_name();
        if names.is_empty() {
            out.extend(configured.iter().cloned());
        } else {
            for name in names {
                let found = configured.iter().find(|b| &b.name == name).ok_or_else(|| {
                    let known = if configured.is_empty() {
                        "(none configured)".to_string()
                    } else {
                        configured
                            .iter()
                            .map(|b| b.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    anyhow::anyhow!(
                        "unknown --budget-name '{}' (configured budgets: {})",
                        name,
                        known
                    )
                })?;
                out.push(found.clone());
            }
            // Sort the selected configured budgets by name so the rendered
            // order depends on the config, not the order the names were typed.
            // One-offs below keep their flag order, which the user chose.
            out.sort_by(|a, b| a.name.cmp(&b.name));
        }

        let window_text = self.budget_window().unwrap_or("1d");
        for spec in self.budget() {
            out.push(parse_budget_spec(spec, window_text)?);
        }

        crate::budget::validate_all(&out)?;
        Ok(out)
    }
}

/// Parse a `--budget <source:amount>` one-off. The window comes from
/// `--budget-window` (default `1d`).
fn parse_budget_spec(spec: &str, window: &str) -> anyhow::Result<crate::budget::Budget> {
    let (source, amount) = spec.split_once(':').ok_or_else(|| {
        anyhow::anyhow!("invalid --budget '{}': expected <source>:<amount>", spec)
    })?;
    let max_cost: f64 = amount.parse().map_err(|_| {
        anyhow::anyhow!("invalid --budget '{}': '{}' is not a number", spec, amount)
    })?;
    let window = crate::budget::BudgetWindow::parse(window)
        .map_err(|e| anyhow::anyhow!("invalid --budget-window '{}': {}", window, e))?;
    let budget = crate::budget::Budget {
        name: format!("cli:{}", source),
        source: source.to_string(),
        window,
        max_cost,
    };
    crate::budget::validate(&budget)?;
    Ok(budget)
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

    /// Align the window to a local calendar bucket instead of a rolling
    /// duration. With this flag `--last` must be a calendar keyword (`1d` =
    /// today, `1w` = the trailing 7 local days, `1mo` = 30): the window runs
    /// from the bucket's local-midnight start to now, so `--calendar --last 1d`
    /// selects exactly the records a `1d` budget evaluates.
    #[arg(long = "calendar")]
    pub calendar: bool,

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

    /// One-off spend budget as `<source>:<amount>`, e.g. `opencode:5.00`.
    /// Repeatable. The window comes from --budget-window (default 1d).
    #[arg(long = "budget")]
    pub budget: Vec<String>,

    /// Window applied to --budget one-offs (e.g. 1d, 7d, 30d, 1w, 1mo).
    #[arg(long = "budget-window")]
    pub budget_window: Option<String>,

    /// Evaluate only these configured [budget.<name>] entries. Repeatable.
    #[arg(long = "budget-name")]
    pub budget_name: Vec<String>,
}

impl BudgetArgs for UsageArgs {
    fn budget(&self) -> &[String] {
        &self.budget
    }
    fn budget_window(&self) -> Option<&str> {
        self.budget_window.as_deref()
    }
    fn budget_name(&self) -> &[String] {
        &self.budget_name
    }
}

impl UsageArgs {
    /// Validate mutually-exclusive flag combinations.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.json && self.csv {
            anyhow::bail!("--json and --csv are mutually exclusive");
        }
        self.validate_window()?;
        Ok(())
    }
}

impl WindowArgs for UsageArgs {
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn calendar(&self) -> bool {
        self.calendar
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

    /// Align both windows to local calendar buckets instead of rolling
    /// durations. With this flag `--last`/`--prev` must be calendar keywords
    /// (`1d` = today, `1w` = the trailing 7 local days, `1mo` = 30): the current
    /// window runs from the bucket's local-midnight start to `now`, and the
    /// previous window is the adjacent bucket immediately before it.
    #[arg(long = "calendar")]
    pub calendar: bool,

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

    /// Parse `--last`/`--prev` as calendar bucket lengths, in local days.
    ///
    /// Only the calendar keywords shared with `budget` (`1d`, `1w`, `1mo`) are
    /// accepted: a rolling duration like `4h` has no calendar meaning and must
    /// be a loud error rather than a silently misaligned window.
    pub fn parse_calendar_windows(&self) -> anyhow::Result<(u32, u32)> {
        let last = match &self.last {
            Some(s) => crate::domain::window::calendar_days(s).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid --last for --calendar: '{}' (expected calendar window: 1d, 1w, 1mo)",
                    s
                )
            })?,
            None => anyhow::bail!("--last is required"),
        };
        let prev = match &self.prev {
            Some(s) => crate::domain::window::calendar_days(s).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid --prev for --calendar: '{}' (expected calendar window: 1d, 1w, 1mo)",
                    s
                )
            })?,
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

    /// Align the window to a local calendar bucket instead of a rolling
    /// duration. With this flag `--last` must be a calendar keyword (`1d` =
    /// today, `1w` = the trailing 7 local days, `1mo` = 30): the window runs
    /// from the bucket's local-midnight start to now, so `--calendar --last 1d`
    /// selects exactly the records a `1d` budget evaluates.
    #[arg(long = "calendar")]
    pub calendar: bool,

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

    /// One-off spend budget as `<source>:<amount>`, e.g. `opencode:5.00`.
    /// Repeatable. The window comes from --budget-window (default 1d).
    #[arg(long = "budget")]
    pub budget: Vec<String>,

    /// Window applied to --budget one-offs (e.g. 1d, 7d, 30d, 1w, 1mo).
    #[arg(long = "budget-window")]
    pub budget_window: Option<String>,

    /// Evaluate only these configured [budget.<name>] entries. Repeatable.
    #[arg(long = "budget-name")]
    pub budget_name: Vec<String>,
}

impl BudgetArgs for ReportArgs {
    fn budget(&self) -> &[String] {
        &self.budget
    }
    fn budget_window(&self) -> Option<&str> {
        self.budget_window.as_deref()
    }
    fn budget_name(&self) -> &[String] {
        &self.budget_name
    }
}

impl ReportArgs {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_window()?;
        if let Some(top) = self.top {
            if top == 0 {
                anyhow::bail!("--top must be at least 1");
            }
        }
        Ok(())
    }
}

impl WindowArgs for ReportArgs {
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn calendar(&self) -> bool {
        self.calendar
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

#[derive(Clone, Copy, Debug, Default, ValueEnum, PartialEq, Eq)]
pub enum ExportFormatArg {
    /// One compact JSON object per line.
    #[default]
    Jsonl,
    /// A single pretty-printed JSON array.
    Json,
    /// Comma-separated, with a header row.
    Csv,
    /// Tab-separated, with a header row.
    Tsv,
}

impl From<ExportFormatArg> for crate::export::ExportFormat {
    fn from(v: ExportFormatArg) -> Self {
        match v {
            ExportFormatArg::Jsonl => Self::Jsonl,
            ExportFormatArg::Json => Self::Json,
            ExportFormatArg::Csv => Self::Csv,
            ExportFormatArg::Tsv => Self::Tsv,
        }
    }
}

impl std::fmt::Display for ExportFormatArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jsonl => write!(f, "jsonl"),
            Self::Json => write!(f, "json"),
            Self::Csv => write!(f, "csv"),
            Self::Tsv => write!(f, "tsv"),
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct ExportArgs {
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

    /// Output encoding. Defaults to jsonl.
    #[arg(long = "format", default_value_t)]
    pub format: ExportFormatArg,

    /// Columns to emit, in order. Comma-separated and/or repeated. Defaults
    /// to every field in canonical order.
    #[arg(long = "fields", value_delimiter = ',')]
    pub fields: Vec<String>,

    /// Export one row per transcript message instead of one row per session.
    /// This is the corpus `search` reads; `--fields` then resolves against the
    /// message field set (`source,session_id,project,model,role,timestamp,text`).
    #[arg(long = "messages")]
    pub messages: bool,

    /// Only include messages with this role (e.g. user, assistant, thinking).
    /// Requires --messages.
    #[arg(long = "role")]
    pub role: Option<String>,
}

impl ExportArgs {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.since.is_some() && self.last.is_some() {
            anyhow::bail!("--since and --last are mutually exclusive");
        }
        if self.role.is_some() && !self.messages {
            anyhow::bail!("--role requires --messages");
        }
        Ok(())
    }
}

impl FilterArgs for ExportArgs {
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

impl SourcePathArgs for ExportArgs {
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

/// The live monitor. It consumes the same filter, window and budget vocabulary
/// as `usage`; the only genuinely new knob is how often it re-reads the sources.
#[derive(Parser, Debug, Clone)]
pub struct WatchArgs {
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

    /// Align the window to a local calendar bucket instead of a rolling
    /// duration, exactly as `usage --calendar` does. The bucket is re-anchored
    /// on every reload, so a monitor left running across midnight rolls into
    /// the new local day.
    #[arg(long = "calendar")]
    pub calendar: bool,

    /// Filter by project path substring.
    #[arg(long = "project")]
    pub project: Option<String>,

    /// Filter by model substring (case-insensitive).
    #[arg(long = "model")]
    pub model: Option<String>,

    /// Filter by source name.
    #[arg(long = "source")]
    pub source: Option<SourceArg>,

    /// Group the table by this dimension: source, project, or model.
    #[arg(long = "group-by", default_value_t)]
    pub group_by: GroupByArg,

    /// Seconds between reloads. Must be at least 1; a zero interval would
    /// busy-loop the sources.
    #[arg(long = "interval", default_value_t = 5)]
    pub interval: u64,

    /// Dump exactly one frame as JSON and exit, instead of monitoring. The
    /// frame is byte-identical to the `usage --json` frame for the same flags.
    #[arg(long = "json")]
    pub json: bool,

    /// One-off spend budget as `<source>:<amount>`, e.g. `opencode:5.00`.
    /// Repeatable. The window comes from --budget-window (default 1d).
    #[arg(long = "budget")]
    pub budget: Vec<String>,

    /// Window applied to --budget one-offs (e.g. 1d, 7d, 30d, 1w, 1mo).
    #[arg(long = "budget-window")]
    pub budget_window: Option<String>,

    /// Evaluate only these configured [budget.<name>] entries. Repeatable.
    #[arg(long = "budget-name")]
    pub budget_name: Vec<String>,
}

impl WatchArgs {
    /// Reject an interval that cannot be honoured. Runs before any source is
    /// read so a bad invocation fails instantly rather than after a disk scan.
    pub fn validate_interval(&self) -> anyhow::Result<()> {
        if self.interval == 0 {
            anyhow::bail!("invalid --interval 0: must be at least 1 second");
        }
        Ok(())
    }
}

impl FilterArgs for WatchArgs {
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

impl SourcePathArgs for WatchArgs {
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

impl BudgetArgs for WatchArgs {
    fn budget(&self) -> &[String] {
        &self.budget
    }
    fn budget_window(&self) -> Option<&str> {
        self.budget_window.as_deref()
    }
    fn budget_name(&self) -> &[String] {
        &self.budget_name
    }
}

impl WindowArgs for WatchArgs {
    fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
    fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }
    fn calendar(&self) -> bool {
        self.calendar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{Budget, BudgetWindow};

    fn configured(name: &str) -> Budget {
        Budget {
            name: name.to_string(),
            source: "opencode".to_string(),
            window: BudgetWindow::parse("7d").unwrap(),
            max_cost: 10.0,
        }
    }

    fn usage_from(argv: &[&str]) -> UsageArgs {
        let mut full = vec!["llmhelper", "usage"];
        full.extend_from_slice(argv);
        match Cli::try_parse_from(full).unwrap().command {
            Command::Usage(args) => args,
            other => panic!("expected usage, got {other:?}"),
        }
    }

    fn diff_from(argv: &[&str]) -> DiffArgs {
        let mut full = vec!["llmhelper", "diff"];
        full.extend_from_slice(argv);
        match Cli::try_parse_from(full).unwrap().command {
            Command::Diff(args) => args,
            other => panic!("expected diff, got {other:?}"),
        }
    }

    #[test]
    fn calendar_windows_parse_the_three_keywords() {
        let args = diff_from(&["--calendar", "--last", "1d", "--prev", "1w"]);
        assert_eq!(args.parse_calendar_windows().unwrap(), (1, 7));
    }

    #[test]
    fn calendar_rejects_a_rolling_duration_for_last() {
        let args = diff_from(&["--calendar", "--last", "4h", "--prev", "1d"]);
        let err = args.parse_calendar_windows().unwrap_err().to_string();
        assert!(err.contains("--last"), "{}", err);
        assert!(err.contains("4h"), "{}", err);
    }

    #[test]
    fn calendar_rejects_a_rolling_duration_for_prev() {
        let args = diff_from(&["--calendar", "--last", "1d", "--prev", "2d"]);
        let err = args.parse_calendar_windows().unwrap_err().to_string();
        assert!(err.contains("--prev"), "{}", err);
        assert!(err.contains("2d"), "{}", err);
    }

    #[test]
    fn calendar_requires_both_windows() {
        let args = diff_from(&["--calendar", "--last", "1d"]);
        assert!(args.parse_calendar_windows().is_err());
    }

    #[test]
    fn budget_flag_parses_source_and_amount() {
        let args = usage_from(&["--budget", "opencode:5.00"]);
        let resolved = args.resolve_budgets(&[]).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].source, "opencode");
        assert_eq!(resolved[0].max_cost, 5.00);
    }

    #[test]
    fn budget_flag_defaults_window_to_one_day() {
        let args = usage_from(&["--budget", "opencode:5.00"]);
        let resolved = args.resolve_budgets(&[]).unwrap();
        assert_eq!(resolved[0].window, BudgetWindow::parse("1d").unwrap());
    }

    #[test]
    fn budget_window_applies_to_one_offs() {
        let args = usage_from(&["--budget", "opencode:5.00", "--budget-window", "30d"]);
        let resolved = args.resolve_budgets(&[]).unwrap();
        assert_eq!(resolved[0].window, BudgetWindow::parse("30d").unwrap());
    }

    #[test]
    fn budget_flag_is_repeatable() {
        let args = usage_from(&["--budget", "opencode:5", "--budget", "omp:2"]);
        let resolved = args.resolve_budgets(&[]).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].source, "opencode");
        assert_eq!(resolved[1].source, "omp");
    }

    #[test]
    fn budget_without_colon_is_rejected() {
        let args = usage_from(&["--budget", "opencode"]);
        let err = args.resolve_budgets(&[]).unwrap_err().to_string();
        assert!(err.contains("expected <source>:<amount>"), "{err}");
    }

    #[test]
    fn budget_with_non_numeric_amount_is_rejected() {
        let args = usage_from(&["--budget", "opencode:abc"]);
        let err = args.resolve_budgets(&[]).unwrap_err().to_string();
        assert!(err.contains("is not a number"), "{err}");
    }

    #[test]
    fn budget_with_unknown_source_is_rejected() {
        let args = usage_from(&["--budget", "bogus:5"]);
        let err = args.resolve_budgets(&[]).unwrap_err().to_string();
        assert!(err.contains("unknown source 'bogus'"), "{err}");
    }

    #[test]
    fn budget_with_non_positive_amount_is_rejected() {
        let args = usage_from(&["--budget", "opencode:0"]);
        let err = args.resolve_budgets(&[]).unwrap_err().to_string();
        assert!(err.contains("positive finite number"), "{err}");
    }

    #[test]
    fn bad_budget_window_is_rejected() {
        let args = usage_from(&["--budget", "opencode:5", "--budget-window", "fortnight"]);
        let err = args.resolve_budgets(&[]).unwrap_err().to_string();
        assert!(err.contains("invalid --budget-window"), "{err}");
    }

    #[test]
    fn no_budget_flags_yields_configured_budgets() {
        let args = usage_from(&[]);
        let resolved = args.resolve_budgets(&[configured("daily")]).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "daily");
    }

    #[test]
    fn budget_name_selects_one_configured_budget() {
        let args = usage_from(&["--budget-name", "weekly"]);
        let resolved = args
            .resolve_budgets(&[configured("daily"), configured("weekly")])
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "weekly");
    }

    #[test]
    fn unknown_budget_name_lists_configured_names() {
        let args = usage_from(&["--budget-name", "nope"]);
        let err = args
            .resolve_budgets(&[configured("daily"), configured("weekly")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown --budget-name 'nope'"), "{err}");
        assert!(err.contains("daily, weekly"), "{err}");
    }

    #[test]
    fn unknown_budget_name_with_nothing_configured_says_so() {
        let args = usage_from(&["--budget-name", "nope"]);
        let err = args.resolve_budgets(&[]).unwrap_err().to_string();
        assert!(err.contains("(none configured)"), "{err}");
    }

    #[test]
    fn named_selection_is_sorted_by_name_not_flag_order() {
        let args = usage_from(&["--budget-name", "weekly", "--budget-name", "daily"]);
        let resolved = args
            .resolve_budgets(&[configured("weekly"), configured("daily")])
            .unwrap();
        assert_eq!(
            resolved.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["daily", "weekly"]
        );
    }

    #[test]
    fn configured_and_one_off_budgets_combine() {
        let args = usage_from(&["--budget", "omp:2"]);
        let resolved = args.resolve_budgets(&[configured("daily")]).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "daily");
        assert_eq!(resolved[1].source, "omp");
    }

    #[test]
    fn report_accepts_the_same_budget_flags() {
        let cli = Cli::try_parse_from([
            "llmhelper",
            "report",
            "--budget",
            "opencode:5",
            "--budget-window",
            "1d",
        ])
        .unwrap();
        match cli.command {
            Command::Report(args) => {
                let resolved = args.resolve_budgets(&[]).unwrap();
                assert_eq!(resolved[0].source, "opencode");
            }
            other => panic!("expected report, got {other:?}"),
        }
    }
}

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub claude_dir: Option<PathBuf>,
    pub opencode_dbs: Option<Vec<PathBuf>>,
    pub omp_dir: Option<PathBuf>,
    pub kilo_dbs: Option<Vec<PathBuf>>,
    pub refresh_interval_seconds: u64,
    pub request_base_url: Option<String>,
    pub request_api_key: Option<String>,
    pub request_default_model: Option<String>,
    pub request_timeout_seconds: Option<u64>,
    pub request_reasoning_fields: Option<Vec<String>>,
    pub request_reasoning: Option<PathBuf>,
    /// Source-scoped spend budgets declared under `[budget.<name>]`, in the
    /// file's declaration order. Empty when the table is absent.
    pub budgets: Vec<crate::budget::Budget>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            claude_dir: None,
            opencode_dbs: None,
            omp_dir: None,
            kilo_dbs: None,
            refresh_interval_seconds: 5,
            request_base_url: None,
            request_api_key: None,
            request_default_model: None,
            request_timeout_seconds: None,
            request_reasoning_fields: None,
            request_reasoning: None,
            budgets: Vec::new(),
        }
    }
}

impl Config {
    /// Default config location: `~/.config/llmhelper/config.toml`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join("llmhelper")
            .join("config.toml")
    }

    /// Load config from the default location, or from `override_path` when
    /// given (e.g. via `--config`). A missing file yields defaults; a
    /// malformed file logs a warning and yields defaults.
    pub fn load_with(override_path: Option<&std::path::Path>) -> Self {
        let path = override_path
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_path);
        if !path.exists() {
            return Self::default();
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warn: cannot read config {:?}: {}", path, e);
                return Self::default();
            }
        };
        let parsed: ConfigTable = match toml::from_str(&content) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("warn: cannot parse config {:?}: {}", path, e);
                return Self::default();
            }
        };
        Self {
            claude_dir: parsed
                .source
                .as_ref()
                .and_then(|s| s.claude.as_ref().and_then(|c| c.dir.clone())),
            opencode_dbs: parsed
                .source
                .as_ref()
                .and_then(|s| s.opencode.as_ref().and_then(|o| o.db.clone())),
            omp_dir: parsed
                .source
                .as_ref()
                .and_then(|s| s.omp.as_ref().and_then(|o| o.dir.clone())),
            kilo_dbs: parsed
                .source
                .as_ref()
                .and_then(|s| s.kilo.as_ref().and_then(|k| k.db.clone())),
            refresh_interval_seconds: parsed
                .ui
                .and_then(|u| u.refresh_interval_seconds)
                .unwrap_or(5),
            request_base_url: parsed.request.as_ref().and_then(|r| r.base_url.clone()),
            request_api_key: parsed.request.as_ref().and_then(|r| r.api_key.clone()),
            request_default_model: parsed
                .request
                .as_ref()
                .and_then(|r| r.default_model.clone()),
            request_timeout_seconds: parsed.request.as_ref().and_then(|r| r.timeout_seconds),
            request_reasoning_fields: parsed
                .request
                .as_ref()
                .and_then(|r| r.reasoning_fields.clone()),
            request_reasoning: parsed
                .request
                .as_ref()
                .and_then(|r| r.reasoning.clone())
                .map(|p| resolve_relative(&path, p)),
            budgets: parsed
                .budget
                .unwrap_or_default()
                .into_iter()
                .map(|(name, b)| {
                    let window = crate::budget::BudgetWindow::parse_or_unparsed(&b.window);
                    crate::budget::Budget {
                        name,
                        source: b.source,
                        window,
                        max_cost: b.max_cost,
                    }
                })
                .collect(),
        }
    }

    /// Load config from the default location.
    pub fn load() -> Self {
        Self::load_with(None)
    }
}

#[derive(serde::Deserialize, Debug, Default)]
struct ConfigTable {
    source: Option<SourceConfig>,
    ui: Option<UiConfig>,
    request: Option<RequestConfig>,
    /// `[budget.<name>]` tables, in declaration order.
    budget: Option<std::collections::BTreeMap<String, BudgetConfig>>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct BudgetConfig {
    source: String,
    window: String,
    max_cost: f64,
}

#[derive(serde::Deserialize, Debug, Default)]
struct SourceConfig {
    claude: Option<ClaudeSourceConfig>,
    opencode: Option<OpenCodeSourceConfig>,
    omp: Option<OmpSourceConfig>,
    kilo: Option<KiloSourceConfig>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct ClaudeSourceConfig {
    dir: Option<PathBuf>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct OpenCodeSourceConfig {
    db: Option<Vec<PathBuf>>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct OmpSourceConfig {
    dir: Option<PathBuf>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct KiloSourceConfig {
    db: Option<Vec<PathBuf>>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct UiConfig {
    refresh_interval_seconds: Option<u64>,
}

#[derive(serde::Deserialize, Debug, Default)]
struct RequestConfig {
    base_url: Option<String>,
    api_key: Option<String>,
    default_model: Option<String>,
    timeout_seconds: Option<u64>,
    reasoning_fields: Option<Vec<String>>,
    reasoning: Option<PathBuf>,
}

/// Resolve a path that appeared inside a config file. A relative path is
/// interpreted relative to the config file's own directory, so a config and
/// the files it references can be moved together; an absolute path is left
/// alone.
fn resolve_relative(config_path: &std::path::Path, candidate: PathBuf) -> PathBuf {
    if candidate.is_absolute() {
        return candidate;
    }
    match config_path.parent() {
        Some(dir) => dir.join(candidate),
        None => candidate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_loads_from_file() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[source.claude]
dir = "/custom/claude"

[source.opencode]
db = ["/custom/opencode.db"]

[source.omp]
dir = "/custom/omp/sessions"

[source.kilo]
db = ["/custom/kilo/kilo.db"]

[ui]
refresh_interval_seconds = 10
"#,
        )
        .unwrap();
        // Test the parser directly rather than relying on dirs crate behavior
        let content = std::fs::read_to_string(&config_path).unwrap();
        let parsed: ConfigTable = toml::from_str(&content).unwrap();
        assert_eq!(
            parsed
                .source
                .as_ref()
                .and_then(|s| s.claude.as_ref().and_then(|c| c.dir.clone()))
                .as_deref(),
            Some(std::path::Path::new("/custom/claude"))
        );
        assert_eq!(
            parsed
                .source
                .as_ref()
                .and_then(|s| s.opencode.as_ref().and_then(|o| o.db.clone()))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parsed
                .source
                .as_ref()
                .and_then(|s| s.omp.as_ref().and_then(|o| o.dir.clone()))
                .as_deref(),
            Some(std::path::Path::new("/custom/omp/sessions"))
        );
        assert_eq!(
            parsed
                .source
                .as_ref()
                .and_then(|s| s.kilo.as_ref().and_then(|k| k.db.clone()))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parsed
                .ui
                .as_ref()
                .and_then(|u| u.refresh_interval_seconds)
                .unwrap(),
            10
        );
    }

    #[test]
    fn config_missing_file_returns_defaults() {
        let default_cfg = Config::default();
        assert!(default_cfg.claude_dir.is_none());
        assert_eq!(default_cfg.refresh_interval_seconds, 5);
    }

    #[test]
    fn budget_table_parses_into_ordered_budgets() {
        let parsed: ConfigTable = toml::from_str(
            r#"
[budget.zebra]
source = "omp"
window = "30d"
max_cost = 40.0

[budget.alpha]
source = "opencode"
window = "1d"
max_cost = 5.0
"#,
        )
        .unwrap();
        let budgets = parsed.budget.unwrap();
        // BTreeMap: declaration order does not matter, name order does.
        let names: Vec<&String> = budgets.keys().collect();
        assert_eq!(names, vec!["alpha", "zebra"]);
        assert_eq!(budgets["alpha"].source, "opencode");
        assert_eq!(budgets["alpha"].window, "1d");
        assert_eq!(budgets["alpha"].max_cost, 5.0);
    }

    #[test]
    fn budget_table_absent_yields_empty() {
        let parsed: ConfigTable = toml::from_str("[ui]\nrefresh_interval_seconds = 5\n").unwrap();
        assert!(parsed.budget.is_none());
    }

    #[test]
    fn config_load_produces_budgets_with_parsed_windows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[budget.daily]
source = "opencode"
window = "1d"
max_cost = 5.0
"#,
        )
        .unwrap();
        let cfg = Config::load_with(Some(&path));
        assert_eq!(cfg.budgets.len(), 1);
        assert_eq!(cfg.budgets[0].name, "daily");
        assert_eq!(
            cfg.budgets[0].window,
            crate::budget::BudgetWindow::Calendar {
                days: 1,
                label: "1d".to_string()
            }
        );
    }

    #[test]
    fn malformed_budget_window_is_carried_for_validation_to_reject() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[budget.bad]
source = "opencode"
window = "soon"
max_cost = 5.0
"#,
        )
        .unwrap();
        let cfg = Config::load_with(Some(&path));
        // Loading succeeds; validation is what rejects it, naming the value.
        assert!(crate::budget::validate_all(&cfg.budgets).is_err());
        let err = crate::budget::validate_all(&cfg.budgets)
            .unwrap_err()
            .to_string();
        assert!(err.contains("soon") && err.contains("bad"), "{}", err);
    }

    #[test]
    fn malformed_budget_source_and_amount_are_rejected_by_validation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[budget.badsrc]
source = "gemini"
window = "1d"
max_cost = 5.0

[budget.badamt]
source = "opencode"
window = "1d"
max_cost = 0.0
"#,
        )
        .unwrap();
        let cfg = Config::load_with(Some(&path));
        let err = crate::budget::validate_all(&cfg.budgets)
            .unwrap_err()
            .to_string();
        // BTreeMap order: badamt before badsrc.
        assert!(
            err.contains("badamt") && err.contains("max_cost"),
            "{}",
            err
        );
    }
}

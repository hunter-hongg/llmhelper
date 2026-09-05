use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub claude_dir: Option<PathBuf>,
    pub opencode_dbs: Option<Vec<PathBuf>>,
    pub omp_dir: Option<PathBuf>,
    pub kilo_dbs: Option<Vec<PathBuf>>,
    pub refresh_interval_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            claude_dir: None,
            opencode_dbs: None,
            omp_dir: None,
            kilo_dbs: None,
            refresh_interval_seconds: 5,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join("llmhelper")
            .join("config.toml");
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
        }
    }

    /// Merge CLI overrides on top of config. CLI flags win.
    pub fn merge(self, cli: &crate::cli::UsageArgs) -> Self {
        Self {
            claude_dir: cli.claude_dir.clone().or(self.claude_dir),
            opencode_dbs: cli.opencode_db.clone().or(self.opencode_dbs),
            omp_dir: cli.omp_dir.clone().or(self.omp_dir),
            kilo_dbs: cli.kilo_db.clone().or(self.kilo_dbs),
            refresh_interval_seconds: self.refresh_interval_seconds,
        }
    }
}

#[derive(serde::Deserialize, Debug, Default)]
struct ConfigTable {
    source: Option<SourceConfig>,
    ui: Option<UiConfig>,
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
}

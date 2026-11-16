//! Configuration file: `$XDG_CONFIG_HOME/stargazer/config.toml` (platform
//! equivalent elsewhere). Every key is optional.
//!
//! Precedence, highest first: command-line flags, environment variables
//! (`STARGAZER_USER`, `STARGAZER_ASCII`, `NO_COLOR`), this file, defaults.
//!
//! ```toml
//! user = "octocat"
//! sort = "stars"          # stars | recent | name | starred
//! ascii = false           # plain-ASCII icons and glyphs
//! theme = "dark"          # dark | light | high-contrast | mono
//! stale_days = 7          # full resync (drops unstarred repos) after N days; 0 = never
//! mouse = true            # mouse wheel and click support
//!
//! [colors]                # optional overrides; any ratatui color name or #rrggbb
//! accent = "cyan"
//! selected_bg = "#303040"
//! ```

use crate::app::SortMode;
use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    Dark,
    Light,
    HighContrast,
    Mono,
}

/// Optional per-role color overrides.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColorOverrides {
    pub accent: Option<String>,
    pub text: Option<String>,
    pub dim: Option<String>,
    pub selected_fg: Option<String>,
    pub selected_bg: Option<String>,
    pub border: Option<String>,
    pub stars: Option<String>,
    pub language: Option<String>,
    pub description: Option<String>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub user: Option<String>,
    pub sort: SortMode,
    pub ascii: bool,
    pub theme: ThemeName,
    pub stale_days: u32,
    pub mouse: bool,
    pub colors: ColorOverrides,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            user: None,
            sort: SortMode::Stars,
            ascii: false,
            theme: ThemeName::Dark,
            stale_days: 7,
            mouse: true,
            colors: ColorOverrides::default(),
        }
    }
}

impl Config {
    /// Default config path for this platform.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", "stargazer")
            .ok_or_else(|| anyhow!("cannot determine a config directory for this platform"))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load `path`; a missing file is the default config, a malformed one an error.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Self::parse(&raw).with_context(|| format!("in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Ok(toml::from_str(raw)?)
    }
}

/// True when an environment flag is set to a truthy value.
pub fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !v.is_empty() && v != "0" && v != "false" && v != "no"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn full_file_parses() {
        let c = Config::parse(
            r##"
            user = "octocat"
            sort = "recent"
            ascii = true
            theme = "high-contrast"
            stale_days = 0
            mouse = false
            [colors]
            accent = "magenta"
            selected_bg = "#202020"
            "##,
        )
        .unwrap();
        assert_eq!(c.user.as_deref(), Some("octocat"));
        assert_eq!(c.sort, SortMode::RecentlyActive);
        assert!(c.ascii);
        assert_eq!(c.theme, ThemeName::HighContrast);
        assert_eq!(c.stale_days, 0);
        assert!(!c.mouse);
        assert_eq!(c.colors.accent.as_deref(), Some("magenta"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("colour = 1").is_err());
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&dir.path().join("none.toml")).unwrap(),
            Config::default()
        );
    }
}

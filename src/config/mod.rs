//! Configuration management.
//!
//! Handles loading and saving Lore configuration from `~/.lore/config.yaml`.
//! Configuration controls which watchers are enabled, auto-linking behavior,
//! and commit message formatting.
//!
//! Note: Repo-level configuration (`.lore/config.yaml` in repo root) is a future
//! enhancement. Currently only global config at `~/.lore/config.yaml` is supported.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Lore configuration settings.
///
/// Controls watcher behavior, auto-linking, and commit integration.
/// Loaded from `~/.lore/config.yaml` when available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// List of enabled watcher names (e.g., "claude-code", "cursor").
    pub watchers: Vec<String>,

    /// Whether to automatically link sessions to commits.
    pub auto_link: bool,

    /// Minimum confidence score (0.0-1.0) required for auto-linking.
    pub auto_link_threshold: f64,

    /// Whether to append session references to commit messages.
    pub commit_footer: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watchers: vec!["claude-code".to_string()],
            auto_link: false,
            auto_link_threshold: 0.7,
            commit_footer: false,
        }
    }
}

impl Config {
    /// Loads configuration from the default config file.
    ///
    /// Returns default configuration if the file does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from_path(&path)
    }

    /// Saves configuration to the default config file.
    ///
    /// Creates the `~/.lore` directory if it does not exist.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        self.save_to_path(&path)
    }

    /// Loads configuration from a specific path.
    ///
    /// Returns default configuration if the file does not exist.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        if content.trim().is_empty() {
            return Ok(Self::default());
        }

        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Saves configuration to a specific path.
    ///
    /// Creates parent directories if they do not exist.
    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let content = serde_yaml::to_string(self).context("Failed to serialize config")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }

    /// Gets a configuration value by key.
    ///
    /// Supported keys:
    /// - `watchers` - comma-separated list of enabled watchers
    /// - `auto_link` - "true" or "false"
    /// - `auto_link_threshold` - float between 0.0 and 1.0
    /// - `commit_footer` - "true" or "false"
    ///
    /// Returns `None` if the key is not recognized.
    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "watchers" => Some(self.watchers.join(",")),
            "auto_link" => Some(self.auto_link.to_string()),
            "auto_link_threshold" => Some(self.auto_link_threshold.to_string()),
            "commit_footer" => Some(self.commit_footer.to_string()),
            _ => None,
        }
    }

    /// Sets a configuration value by key.
    ///
    /// Supported keys:
    /// - `watchers` - comma-separated list of enabled watchers
    /// - `auto_link` - "true" or "false"
    /// - `auto_link_threshold` - float between 0.0 and 1.0 (inclusive)
    /// - `commit_footer` - "true" or "false"
    ///
    /// Returns an error if the key is not recognized or the value is invalid.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "watchers" => {
                self.watchers = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "auto_link" => {
                self.auto_link = parse_bool(value)
                    .with_context(|| format!("Invalid value for auto_link: '{value}'"))?;
            }
            "auto_link_threshold" => {
                let threshold: f64 = value
                    .parse()
                    .with_context(|| format!("Invalid value for auto_link_threshold: '{value}'"))?;
                if !(0.0..=1.0).contains(&threshold) {
                    bail!("auto_link_threshold must be between 0.0 and 1.0, got {threshold}");
                }
                self.auto_link_threshold = threshold;
            }
            "commit_footer" => {
                self.commit_footer = parse_bool(value)
                    .with_context(|| format!("Invalid value for commit_footer: '{value}'"))?;
            }
            _ => {
                bail!("Unknown configuration key: '{key}'");
            }
        }
        Ok(())
    }

    /// Returns the path to the configuration file.
    ///
    /// The configuration file is located at `~/.lore/config.yaml`.
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".lore");

        Ok(config_dir.join("config.yaml"))
    }

    /// Returns the list of valid configuration keys.
    pub fn valid_keys() -> &'static [&'static str] {
        &[
            "watchers",
            "auto_link",
            "auto_link_threshold",
            "commit_footer",
        ]
    }
}

/// Parses a boolean value from a string.
///
/// Accepts "true", "false", "1", "0", "yes", "no" (case-insensitive).
fn parse_bool(value: &str) -> Result<bool> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => bail!("Expected 'true' or 'false', got '{value}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.watchers, vec!["claude-code".to_string()]);
        assert!(!config.auto_link);
        assert!((config.auto_link_threshold - 0.7).abs() < f64::EPSILON);
        assert!(!config.commit_footer);
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.yaml");

        let config = Config::load_from_path(&path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("config.yaml");

        let config = Config {
            auto_link: true,
            auto_link_threshold: 0.8,
            watchers: vec!["claude-code".to_string(), "cursor".to_string()],
            ..Default::default()
        };

        config.save_to_path(&path).unwrap();

        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn test_save_creates_parent_directories() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir
            .path()
            .join("nested")
            .join("dir")
            .join("config.yaml");

        let config = Config::default();
        config.save_to_path(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_get_watchers() {
        let config = Config {
            watchers: vec!["claude-code".to_string(), "cursor".to_string()],
            ..Default::default()
        };

        assert_eq!(
            config.get("watchers"),
            Some("claude-code,cursor".to_string())
        );
    }

    #[test]
    fn test_get_auto_link() {
        let config = Config {
            auto_link: true,
            ..Default::default()
        };

        assert_eq!(config.get("auto_link"), Some("true".to_string()));
    }

    #[test]
    fn test_get_auto_link_threshold() {
        let config = Config::default();
        assert_eq!(config.get("auto_link_threshold"), Some("0.7".to_string()));
    }

    #[test]
    fn test_get_commit_footer() {
        let config = Config::default();
        assert_eq!(config.get("commit_footer"), Some("false".to_string()));
    }

    #[test]
    fn test_get_unknown_key() {
        let config = Config::default();
        assert_eq!(config.get("unknown_key"), None);
    }

    #[test]
    fn test_set_watchers() {
        let mut config = Config::default();
        config
            .set("watchers", "claude-code, cursor, copilot")
            .unwrap();

        assert_eq!(
            config.watchers,
            vec![
                "claude-code".to_string(),
                "cursor".to_string(),
                "copilot".to_string()
            ]
        );
    }

    #[test]
    fn test_set_auto_link() {
        let mut config = Config::default();

        config.set("auto_link", "true").unwrap();
        assert!(config.auto_link);

        config.set("auto_link", "false").unwrap();
        assert!(!config.auto_link);

        config.set("auto_link", "yes").unwrap();
        assert!(config.auto_link);

        config.set("auto_link", "no").unwrap();
        assert!(!config.auto_link);
    }

    #[test]
    fn test_set_auto_link_threshold() {
        let mut config = Config::default();

        config.set("auto_link_threshold", "0.5").unwrap();
        assert!((config.auto_link_threshold - 0.5).abs() < f64::EPSILON);

        config.set("auto_link_threshold", "0.0").unwrap();
        assert!((config.auto_link_threshold - 0.0).abs() < f64::EPSILON);

        config.set("auto_link_threshold", "1.0").unwrap();
        assert!((config.auto_link_threshold - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_auto_link_threshold_invalid_range() {
        let mut config = Config::default();

        assert!(config.set("auto_link_threshold", "-0.1").is_err());
        assert!(config.set("auto_link_threshold", "1.1").is_err());
        assert!(config.set("auto_link_threshold", "2.0").is_err());
    }

    #[test]
    fn test_set_auto_link_threshold_invalid_format() {
        let mut config = Config::default();

        assert!(config.set("auto_link_threshold", "not_a_number").is_err());
    }

    #[test]
    fn test_set_commit_footer() {
        let mut config = Config::default();

        config.set("commit_footer", "true").unwrap();
        assert!(config.commit_footer);

        config.set("commit_footer", "false").unwrap();
        assert!(!config.commit_footer);
    }

    #[test]
    fn test_set_unknown_key() {
        let mut config = Config::default();

        assert!(config.set("unknown_key", "value").is_err());
    }

    #[test]
    fn test_set_invalid_bool() {
        let mut config = Config::default();

        assert!(config.set("auto_link", "maybe").is_err());
    }

    #[test]
    fn test_load_empty_file_returns_default() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("config.yaml");

        fs::write(&path, "").unwrap();

        let config = Config::load_from_path(&path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn test_valid_keys() {
        let keys = Config::valid_keys();
        assert!(keys.contains(&"watchers"));
        assert!(keys.contains(&"auto_link"));
        assert!(keys.contains(&"auto_link_threshold"));
        assert!(keys.contains(&"commit_footer"));
    }

    #[test]
    fn test_parse_bool() {
        assert!(parse_bool("true").unwrap());
        assert!(parse_bool("TRUE").unwrap());
        assert!(parse_bool("True").unwrap());
        assert!(parse_bool("1").unwrap());
        assert!(parse_bool("yes").unwrap());
        assert!(parse_bool("YES").unwrap());

        assert!(!parse_bool("false").unwrap());
        assert!(!parse_bool("FALSE").unwrap());
        assert!(!parse_bool("False").unwrap());
        assert!(!parse_bool("0").unwrap());
        assert!(!parse_bool("no").unwrap());
        assert!(!parse_bool("NO").unwrap());

        assert!(parse_bool("invalid").is_err());
    }
}

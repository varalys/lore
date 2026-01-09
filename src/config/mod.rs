//! Configuration management.
//!
//! Handles loading and saving Lore configuration from `~/.lore/config.yaml`.
//!
//! Note: Configuration options are planned for a future release. Currently
//! this module provides path information only. The Config struct and its
//! methods are preserved for future use.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

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

    /// Unique machine identifier (UUID) for cloud sync deduplication.
    ///
    /// Auto-generated on first access via `get_or_create_machine_id()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,

    /// Human-readable machine name.
    ///
    /// Defaults to hostname if not set. Can be customized by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watchers: vec!["claude-code".to_string()],
            auto_link: false,
            auto_link_threshold: 0.7,
            commit_footer: false,
            machine_id: None,
            machine_name: None,
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

        let config: Config = serde_saphyr::from_str(&content)
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

        let content = serde_saphyr::to_string(self).context("Failed to serialize config")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }

    /// Returns the machine UUID, generating and saving a new one if needed.
    ///
    /// If no machine_id exists in config, generates a new UUIDv4 and saves
    /// it to the config file. This ensures a consistent machine identifier
    /// across sessions for cloud sync deduplication.
    pub fn get_or_create_machine_id(&mut self) -> Result<String> {
        if let Some(ref id) = self.machine_id {
            return Ok(id.clone());
        }

        let id = Uuid::new_v4().to_string();
        self.machine_id = Some(id.clone());
        self.save()?;
        Ok(id)
    }

    /// Returns the machine name.
    ///
    /// If a custom machine_name is set, returns that. Otherwise returns
    /// the system hostname. Returns "unknown" if hostname cannot be determined.
    pub fn get_machine_name(&self) -> String {
        if let Some(ref name) = self.machine_name {
            return name.clone();
        }

        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Sets a custom machine name and saves the configuration.
    ///
    /// The machine name is a human-readable identifier for this machine,
    /// displayed in session listings and useful for identifying which
    /// machine created a session.
    pub fn set_machine_name(&mut self, name: &str) -> Result<()> {
        self.machine_name = Some(name.to_string());
        self.save()
    }

    /// Gets a configuration value by key.
    ///
    /// Supported keys:
    /// - `watchers` - comma-separated list of enabled watchers
    /// - `auto_link` - "true" or "false"
    /// - `auto_link_threshold` - float between 0.0 and 1.0
    /// - `commit_footer` - "true" or "false"
    /// - `machine_id` - the machine UUID (read-only, auto-generated)
    /// - `machine_name` - human-readable machine name
    ///
    /// Returns `None` if the key is not recognized.
    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "watchers" => Some(self.watchers.join(",")),
            "auto_link" => Some(self.auto_link.to_string()),
            "auto_link_threshold" => Some(self.auto_link_threshold.to_string()),
            "commit_footer" => Some(self.commit_footer.to_string()),
            "machine_id" => self.machine_id.clone(),
            "machine_name" => Some(self.get_machine_name()),
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
    /// - `machine_name` - human-readable machine name
    ///
    /// Note: `machine_id` cannot be set manually; it is auto-generated.
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
            "machine_name" => {
                self.machine_name = Some(value.to_string());
            }
            "machine_id" => {
                bail!("machine_id cannot be set manually; it is auto-generated");
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
            "machine_id",
            "machine_name",
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
        assert!(config.machine_id.is_none());
        assert!(config.machine_name.is_none());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("config.yaml");

        let config = Config {
            auto_link: true,
            auto_link_threshold: 0.8,
            watchers: vec!["claude-code".to_string(), "cursor".to_string()],
            machine_id: Some("test-uuid".to_string()),
            machine_name: Some("test-name".to_string()),
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
    fn test_load_returns_default_for_missing_or_empty_file() {
        let temp_dir = TempDir::new().unwrap();

        // Nonexistent file returns default
        let nonexistent = temp_dir.path().join("nonexistent.yaml");
        let config = Config::load_from_path(&nonexistent).unwrap();
        assert_eq!(config, Config::default());

        // Empty file returns default
        let empty = temp_dir.path().join("empty.yaml");
        fs::write(&empty, "").unwrap();
        let config = Config::load_from_path(&empty).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn test_get_returns_expected_values() {
        let config = Config {
            watchers: vec!["claude-code".to_string(), "cursor".to_string()],
            auto_link: true,
            auto_link_threshold: 0.85,
            commit_footer: true,
            machine_id: Some("test-uuid".to_string()),
            machine_name: Some("test-machine".to_string()),
        };

        assert_eq!(
            config.get("watchers"),
            Some("claude-code,cursor".to_string())
        );
        assert_eq!(config.get("auto_link"), Some("true".to_string()));
        assert_eq!(config.get("auto_link_threshold"), Some("0.85".to_string()));
        assert_eq!(config.get("commit_footer"), Some("true".to_string()));
        assert_eq!(config.get("machine_id"), Some("test-uuid".to_string()));
        assert_eq!(config.get("machine_name"), Some("test-machine".to_string()));
        assert_eq!(config.get("unknown_key"), None);
    }

    #[test]
    fn test_set_updates_values() {
        let mut config = Config::default();

        // Set watchers with whitespace trimming
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

        // Set boolean values with different formats
        config.set("auto_link", "true").unwrap();
        assert!(config.auto_link);
        config.set("auto_link", "no").unwrap();
        assert!(!config.auto_link);

        config.set("commit_footer", "yes").unwrap();
        assert!(config.commit_footer);

        // Set threshold
        config.set("auto_link_threshold", "0.5").unwrap();
        assert!((config.auto_link_threshold - 0.5).abs() < f64::EPSILON);

        // Set machine name
        config.set("machine_name", "dev-workstation").unwrap();
        assert_eq!(config.machine_name, Some("dev-workstation".to_string()));
    }

    #[test]
    fn test_set_validates_threshold_range() {
        let mut config = Config::default();

        // Valid boundary values
        config.set("auto_link_threshold", "0.0").unwrap();
        assert!((config.auto_link_threshold - 0.0).abs() < f64::EPSILON);
        config.set("auto_link_threshold", "1.0").unwrap();
        assert!((config.auto_link_threshold - 1.0).abs() < f64::EPSILON);

        // Invalid values
        assert!(config.set("auto_link_threshold", "-0.1").is_err());
        assert!(config.set("auto_link_threshold", "1.1").is_err());
        assert!(config.set("auto_link_threshold", "not_a_number").is_err());
    }

    #[test]
    fn test_set_rejects_invalid_input() {
        let mut config = Config::default();

        // Unknown key
        assert!(config.set("unknown_key", "value").is_err());

        // Invalid boolean
        assert!(config.set("auto_link", "maybe").is_err());

        // machine_id cannot be set manually
        let result = config.set("machine_id", "some-uuid");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot be set manually"));
    }

    #[test]
    fn test_parse_bool_accepts_multiple_formats() {
        // Truthy values
        assert!(parse_bool("true").unwrap());
        assert!(parse_bool("TRUE").unwrap());
        assert!(parse_bool("1").unwrap());
        assert!(parse_bool("yes").unwrap());
        assert!(parse_bool("YES").unwrap());

        // Falsy values
        assert!(!parse_bool("false").unwrap());
        assert!(!parse_bool("FALSE").unwrap());
        assert!(!parse_bool("0").unwrap());
        assert!(!parse_bool("no").unwrap());

        // Invalid
        assert!(parse_bool("invalid").is_err());
    }

    #[test]
    fn test_machine_name_fallback_to_hostname() {
        let config = Config::default();
        let name = config.get_machine_name();
        // Should return hostname or "unknown", never empty
        assert!(!name.is_empty());
    }

    #[test]
    fn test_machine_identity_yaml_serialization() {
        // When not set, machine_id and machine_name are omitted from YAML
        let config = Config::default();
        let yaml = serde_saphyr::to_string(&config).unwrap();
        assert!(!yaml.contains("machine_id"));
        assert!(!yaml.contains("machine_name"));

        // When set, they are included
        let config = Config {
            machine_id: Some("uuid-1234".to_string()),
            machine_name: Some("my-machine".to_string()),
            ..Default::default()
        };
        let yaml = serde_saphyr::to_string(&config).unwrap();
        assert!(yaml.contains("machine_id"));
        assert!(yaml.contains("machine_name"));
    }
}

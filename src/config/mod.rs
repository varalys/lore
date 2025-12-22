//! Configuration management.
//!
//! Handles loading and saving Lore configuration from `~/.lore/config.yaml`.
//! Configuration controls which watchers are enabled, auto-linking behavior,
//! and commit message formatting.
//!
//! Note: Full configuration persistence is not yet implemented.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Lore configuration settings.
///
/// Controls watcher behavior, auto-linking, and commit integration.
/// Loaded from `~/.lore/config.yaml` when available.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

impl Config {
    /// Loads configuration from the default config file.
    ///
    /// Returns default configuration if the file does not exist.
    #[allow(dead_code)]
    pub fn load() -> Result<Self> {
        // TODO: Load from ~/.lore/config.yaml
        Ok(Self::default())
    }

    /// Returns the path to the configuration file.
    ///
    /// The configuration file is located at `~/.lore/config.yaml`.
    #[allow(dead_code)]
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".lore");

        Ok(config_dir.join("config.yaml"))
    }
}

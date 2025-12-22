//! Configuration management
//!
//! TODO: Implement config file loading/saving

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Enabled watchers
    pub watchers: Vec<String>,

    /// Auto-link settings
    pub auto_link: bool,
    pub auto_link_threshold: f64,

    /// Commit footer settings
    pub commit_footer: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        // TODO: Load from ~/.lore/config.yaml
        Ok(Self::default())
    }

    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".lore");

        Ok(config_dir.join("config.yaml"))
    }
}

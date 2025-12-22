//! Config command - view and manage Lore configuration.
//!
//! Provides subcommands to show, get, and set configuration values.
//! Configuration is stored in `~/.lore/config.yaml`.

use anyhow::{bail, Result};
use clap::Subcommand;
use colored::Colorize;

use crate::config::Config;
use crate::storage::db::default_db_path;

/// Arguments for the config command.
#[derive(clap::Args)]
pub struct Args {
    /// The config subcommand to run.
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,
}

/// Available config subcommands.
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Get a configuration value
    Get {
        /// The configuration key to retrieve.
        key: String,
    },
    /// Set a configuration value
    Set {
        /// The configuration key to set.
        key: String,
        /// The value to assign.
        value: String,
    },
}

/// Executes the config command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        Some(ConfigCommand::Show) | None => show_config(),
        Some(ConfigCommand::Get { key }) => get_config(&key),
        Some(ConfigCommand::Set { key, value }) => set_config(&key, &value),
    }
}

/// Displays all configuration values.
fn show_config() -> Result<()> {
    let config = Config::load()?;

    println!("{}", "Lore Configuration".bold());
    println!();

    let db_path = default_db_path()?;
    let config_path = Config::config_path()?;

    println!("  {}     {}", "Database:".dimmed(), db_path.display());
    println!("  {}  {}", "Config file:".dimmed(), config_path.display());

    println!();
    println!("{}:", "Settings".bold());
    println!(
        "  {:<22} {}",
        "watchers:".dimmed(),
        config.watchers.join(",")
    );
    println!(
        "  {:<22} {}",
        "auto_link:".dimmed(),
        config.auto_link
    );
    println!(
        "  {:<22} {}",
        "auto_link_threshold:".dimmed(),
        config.auto_link_threshold
    );
    println!(
        "  {:<22} {}",
        "commit_footer:".dimmed(),
        config.commit_footer
    );

    Ok(())
}

/// Gets and prints a specific configuration value.
fn get_config(key: &str) -> Result<()> {
    let config = Config::load()?;

    match config.get(key) {
        Some(value) => {
            println!("{value}");
        }
        None => {
            let valid_keys = Config::valid_keys().join(", ");
            bail!("Unknown configuration key: '{key}'. Valid keys are: {valid_keys}");
        }
    }

    Ok(())
}

/// Sets a configuration value and persists it.
fn set_config(key: &str, value: &str) -> Result<()> {
    let mut config = Config::load()?;

    config.set(key, value)?;
    config.save()?;

    println!("Set {key} = {value}");

    Ok(())
}

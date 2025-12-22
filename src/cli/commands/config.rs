//! Config command - view and manage Lore configuration.
//!
//! Provides subcommands to show, get, and set configuration values.
//! Configuration is stored in `~/.lore/config.yaml`.

use anyhow::Result;
use clap::Subcommand;
use colored::Colorize;

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

fn show_config() -> Result<()> {
    println!("{}", "Lore Configuration".bold());
    println!();

    let db_path = default_db_path()?;
    println!("  {}  {}", "Database:".dimmed(), db_path.display());

    let claude_dir = dirs::home_dir()
        .map(|h| h.join(".claude"))
        .unwrap_or_default();
    println!(
        "  {}  {}",
        "Claude Code:".dimmed(),
        claude_dir.display()
    );

    println!();
    println!("{}", "Watchers:".bold());
    println!("  {} claude-code", "✓".green());
    println!("  {} cursor (not implemented)", "○".dimmed());
    println!("  {} copilot (not implemented)", "○".dimmed());

    Ok(())
}

fn get_config(key: &str) -> Result<()> {
    // TODO: Implement config storage
    println!(
        "{}",
        format!("Config key '{key}' not found").yellow()
    );
    Ok(())
}

fn set_config(key: &str, value: &str) -> Result<()> {
    // TODO: Implement config storage
    println!(
        "{}",
        "Config storage not yet implemented".yellow()
    );
    println!("Would set {key} = {value}");
    Ok(())
}

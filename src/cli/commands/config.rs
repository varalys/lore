//! Config command - manage configuration

use anyhow::Result;
use clap::Subcommand;
use colored::Colorize;

use crate::storage::db::default_db_path;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Get a configuration value
    Get { key: String },
    /// Set a configuration value
    Set { key: String, value: String },
}

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
        format!("Config key '{}' not found", key).yellow()
    );
    Ok(())
}

fn set_config(key: &str, value: &str) -> Result<()> {
    // TODO: Implement config storage
    println!(
        "{}",
        "Config storage not yet implemented".yellow()
    );
    println!("Would set {} = {}", key, value);
    Ok(())
}

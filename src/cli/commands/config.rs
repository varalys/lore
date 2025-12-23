//! Config command - view and manage Lore configuration.
//!
//! Provides subcommands to show, get, and set configuration values.
//! Configuration is stored in `~/.lore/config.yaml`.

use anyhow::{bail, Result};
use clap::Subcommand;
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::config::Config;
use crate::storage::db::default_db_path;

/// Arguments for the config command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore config                            Show all settings\n    \
    lore config show                       Show all settings\n    \
    lore config get auto_link              Get a specific setting\n    \
    lore config set auto_link true         Enable a setting\n    \
    lore config set auto_link_threshold 0.7  Set threshold to 70%")]
pub struct Args {
    /// Config subcommand (defaults to 'show')
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text", global = true)]
    pub format: OutputFormat,
}

/// Available config subcommands.
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Display all configuration settings
    #[command(long_about = "Shows all current configuration settings including\n\
        database path, config file location, and all options.")]
    Show,

    /// Get a single configuration value
    #[command(long_about = "Retrieves the current value of a specific configuration key.\n\
        Valid keys: watchers, auto_link, auto_link_threshold, commit_footer")]
    Get {
        /// Configuration key to retrieve
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// Set a configuration value
    #[command(long_about = "Sets a configuration value and saves it to the config file.\n\
        Valid keys: watchers, auto_link, auto_link_threshold, commit_footer")]
    Set {
        /// Configuration key to set
        #[arg(value_name = "KEY")]
        key: String,

        /// Value to assign to the key
        #[arg(value_name = "VALUE")]
        value: String,
    },
}

/// JSON output structure for config show.
#[derive(Serialize)]
struct ConfigOutput {
    database_path: String,
    config_path: String,
    settings: ConfigSettings,
}

/// Config settings for JSON output.
#[derive(Serialize)]
struct ConfigSettings {
    watchers: Vec<String>,
    auto_link: bool,
    auto_link_threshold: f64,
    commit_footer: bool,
}

/// JSON output for config get.
#[derive(Serialize)]
struct ConfigGetOutput {
    key: String,
    value: String,
}

/// Executes the config command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        Some(ConfigCommand::Show) | None => show_config(args.format),
        Some(ConfigCommand::Get { key }) => get_config(&key, args.format),
        Some(ConfigCommand::Set { key, value }) => set_config(&key, &value, args.format),
    }
}

/// Displays all configuration values.
fn show_config(format: OutputFormat) -> Result<()> {
    let config = Config::load()?;
    let db_path = default_db_path()?;
    let config_path = Config::config_path()?;

    match format {
        OutputFormat::Json => {
            let output = ConfigOutput {
                database_path: db_path.display().to_string(),
                config_path: config_path.display().to_string(),
                settings: ConfigSettings {
                    watchers: config.watchers,
                    auto_link: config.auto_link,
                    auto_link_threshold: config.auto_link_threshold,
                    commit_footer: config.commit_footer,
                },
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!("{}", "Lore Configuration".bold());
            println!();

            println!("  {}     {}", "Database:".dimmed(), db_path.display());
            println!("  {}  {}", "Config file:".dimmed(), config_path.display());

            println!();
            println!("{}:", "Settings".bold());
            println!(
                "  {:<22} {}",
                "watchers:".dimmed(),
                config.watchers.join(",")
            );
            println!("  {:<22} {}", "auto_link:".dimmed(), config.auto_link);
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
        }
    }

    Ok(())
}

/// Gets and prints a specific configuration value.
fn get_config(key: &str, format: OutputFormat) -> Result<()> {
    let config = Config::load()?;

    match config.get(key) {
        Some(value) => match format {
            OutputFormat::Json => {
                let output = ConfigGetOutput {
                    key: key.to_string(),
                    value,
                };
                let json = serde_json::to_string_pretty(&output)?;
                println!("{json}");
            }
            OutputFormat::Text | OutputFormat::Markdown => {
                println!("{value}");
            }
        },
        None => {
            let valid_keys = Config::valid_keys().join(", ");
            bail!("Unknown configuration key: '{key}'. Valid keys are: {valid_keys}");
        }
    }

    Ok(())
}

/// JSON output for config set.
#[derive(Serialize)]
struct ConfigSetOutput {
    key: String,
    value: String,
    success: bool,
}

/// Sets a configuration value and persists it.
fn set_config(key: &str, value: &str, format: OutputFormat) -> Result<()> {
    let mut config = Config::load()?;

    config.set(key, value)?;
    config.save()?;

    match format {
        OutputFormat::Json => {
            let output = ConfigSetOutput {
                key: key.to_string(),
                value: value.to_string(),
                success: true,
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!("Set {key} = {value}");
        }
    }

    Ok(())
}

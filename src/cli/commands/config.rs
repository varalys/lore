//! Config command - view Lore configuration paths.
//!
//! Shows database and configuration file locations.
//! Configuration options will be added in a future release.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::config::Config;
use crate::storage::db::default_db_path;

/// Arguments for the config command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore config              Show configuration paths\n    \
    lore config --format json  Output as JSON")]
pub struct Args {
    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for config.
#[derive(Serialize)]
struct ConfigOutput {
    database_path: String,
    config_path: String,
    config_exists: bool,
}

/// Executes the config command.
pub fn run(args: Args) -> Result<()> {
    let db_path = default_db_path()?;
    let config_path = Config::config_path()?;
    let config_exists = config_path.exists();

    match args.format {
        OutputFormat::Json => {
            let output = ConfigOutput {
                database_path: db_path.display().to_string(),
                config_path: config_path.display().to_string(),
                config_exists,
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!("{}", "Lore Configuration".bold());
            println!();
            println!("  {}     {}", "Database:".dimmed(), db_path.display());
            print!("  {}  {}", "Config file:".dimmed(), config_path.display());
            if !config_exists {
                print!(" {}", "(not created)".dimmed());
            }
            println!();
        }
    }

    Ok(())
}

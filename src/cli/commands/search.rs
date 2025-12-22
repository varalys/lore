//! Search command - search session content.
//!
//! Provides full-text search across session messages and metadata.
//! Note: Full-text search is not yet implemented.

use anyhow::Result;
use colored::Colorize;

/// Arguments for the search command.
#[derive(clap::Args)]
pub struct Args {
    /// The text to search for in session content.
    pub query: String,

    /// Maximum number of results to return.
    #[arg(short, long, default_value = "10")]
    pub limit: usize,
}

/// Executes the search command.
///
/// Note: Full-text search is not yet implemented.
pub fn run(args: Args) -> Result<()> {
    // TODO: Implement full-text search
    println!(
        "{}",
        "Search command not yet implemented".yellow()
    );
    println!("Would search for: {}", args.query);
    println!();
    println!(
        "{}",
        "Full-text search requires building a search index.".dimmed()
    );
    println!(
        "{}",
        "For now, use 'lore sessions' to list and 'lore show' to view.".dimmed()
    );
    Ok(())
}

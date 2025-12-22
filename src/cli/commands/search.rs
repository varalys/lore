//! Search command - search sessions

use anyhow::Result;
use colored::Colorize;

#[derive(clap::Args)]
pub struct Args {
    /// Search query
    pub query: String,

    /// Limit results
    #[arg(short, long, default_value = "10")]
    pub limit: usize,
}

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

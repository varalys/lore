//! Unlink command - remove session-to-commit links.
//!
//! Removes associations between sessions and commits. Can unlink
//! a session from all commits or from a specific commit.
//!
//! Note: Unlink functionality is not yet implemented.

use anyhow::Result;
use colored::Colorize;

/// Arguments for the unlink command.
#[derive(clap::Args)]
pub struct Args {
    /// Session ID prefix to unlink.
    pub session: String,

    /// Specific commit SHA to unlink from. If not provided, removes all links.
    #[arg(long)]
    pub commit: Option<String>,
}

/// Executes the unlink command.
///
/// Note: Unlink functionality is not yet implemented.
pub fn run(args: Args) -> Result<()> {
    // TODO: Implement unlinking
    println!(
        "{}",
        "Unlink command not yet implemented".yellow()
    );
    println!("Would unlink session {} from commit {:?}", args.session, args.commit);
    Ok(())
}

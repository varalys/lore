//! Unlink command - remove session links

use anyhow::Result;
use colored::Colorize;

#[derive(clap::Args)]
pub struct Args {
    /// Session ID to unlink
    pub session: String,

    /// Commit SHA to unlink from
    #[arg(long)]
    pub commit: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    // TODO: Implement unlinking
    println!(
        "{}",
        "Unlink command not yet implemented".yellow()
    );
    println!("Would unlink session {} from commit {:?}", args.session, args.commit);
    Ok(())
}

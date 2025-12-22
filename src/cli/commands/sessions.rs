//! Sessions command - list and filter sessions.
//!
//! Displays a list of imported sessions with filtering options.
//! Sessions can be filtered by working directory and output in
//! table or JSON format.

use anyhow::Result;
use colored::Colorize;

use crate::storage::Database;

/// Arguments for the sessions command.
#[derive(clap::Args)]
pub struct Args {
    /// Filter to sessions in this directory (prefix match).
    #[arg(short, long)]
    pub repo: Option<String>,

    /// Maximum number of sessions to display.
    #[arg(short, long, default_value = "20")]
    pub limit: usize,

    /// Output format: "table" or "json".
    #[arg(short, long, default_value = "table")]
    pub format: String,
}

/// Executes the sessions command.
///
/// Lists sessions from the database, optionally filtered by
/// working directory prefix.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Resolve repo path if provided
    let working_dir = args.repo.map(|r| {
        if r == "." {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| r)
        } else {
            r
        }
    });

    let sessions = db.list_sessions(args.limit, working_dir.as_deref())?;

    if sessions.is_empty() {
        println!("{}", "No sessions found.".dimmed());
        println!();
        println!("Run 'lore import' to import sessions from Claude Code.");
        return Ok(());
    }

    match args.format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&sessions)?;
            println!("{json}");
        }
        _ => {
            println!(
                "{}",
                format!(
                    "{:8}  {:14}  {:10}  {:12}  {}",
                    "ID", "STARTED", "MESSAGES", "BRANCH", "DIRECTORY"
                )
                .bold()
            );

            for session in sessions {
                let id_short = &session.id.to_string()[..8];
                let started = session.started_at.format("%Y-%m-%d %H:%M").to_string();
                let branch = session.git_branch.as_deref().unwrap_or("-");
                let dir = session
                    .working_directory
                    .split('/')
                    .next_back()
                    .unwrap_or(&session.working_directory);

                println!(
                    "{}  {}  {:>10}  {:12}  {}",
                    id_short.cyan(),
                    started.dimmed(),
                    session.message_count,
                    branch.yellow(),
                    dir
                );
            }
        }
    }

    Ok(())
}

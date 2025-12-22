//! Sessions command - list and filter sessions

use anyhow::Result;
use colored::Colorize;

use crate::storage::Database;

#[derive(clap::Args)]
pub struct Args {
    /// Filter to sessions in this directory
    #[arg(short, long)]
    pub repo: Option<String>,

    /// Maximum number of sessions to show
    #[arg(short, long, default_value = "20")]
    pub limit: usize,

    /// Output format (table, json)
    #[arg(short, long, default_value = "table")]
    pub format: String,
}

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
            println!("{}", json);
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
                    .last()
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

//! Sessions command - list and filter sessions.
//!
//! Displays a list of imported sessions with filtering options.
//! Sessions can be filtered by working directory and output in
//! text, JSON, or markdown format.

use anyhow::Result;
use colored::Colorize;

use crate::cli::OutputFormat;
use crate::storage::Database;

/// Arguments for the sessions command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore sessions                  List recent sessions (default 20)\n    \
    lore sessions --limit 50       Show up to 50 sessions\n    \
    lore sessions --repo .         Filter to current directory\n    \
    lore sessions --repo /path     Filter to specific path\n    \
    lore sessions --format json    Output as JSON")]
pub struct Args {
    /// Filter to sessions in this directory (prefix match)
    #[arg(short, long, value_name = "PATH")]
    #[arg(long_help = "Filter sessions to those with a working directory matching\n\
        this path prefix. Use '.' for the current directory.")]
    pub repo: Option<String>,

    /// Maximum number of sessions to display
    #[arg(short, long, default_value = "20", value_name = "N")]
    pub limit: usize,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
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

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&sessions)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            // Column widths for consistent alignment
            const ID_WIDTH: usize = 8;
            const STARTED_WIDTH: usize = 16;
            const MESSAGES_WIDTH: usize = 8;
            const BRANCH_WIDTH: usize = 12;

            println!(
                "{}",
                format!(
                    "{:<ID_WIDTH$}  {:<STARTED_WIDTH$}  {:>MESSAGES_WIDTH$}  {:<BRANCH_WIDTH$}  {}",
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
                    "{:<ID_WIDTH$}  {:<STARTED_WIDTH$}  {:>MESSAGES_WIDTH$}  {:<BRANCH_WIDTH$}  {}",
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

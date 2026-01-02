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
    #[arg(
        long_help = "Filter sessions to those with a working directory matching\n\
        this path prefix. Use '.' for the current directory."
    )]
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
            const BRANCH_WIDTH: usize = 24;

            println!(
                "{}",
                format!(
                    "{:<ID_WIDTH$}  {:<STARTED_WIDTH$}  {:>MESSAGES_WIDTH$}  {:<BRANCH_WIDTH$}  {}",
                    "ID", "STARTED", "MESSAGES", "BRANCH", "DIRECTORY"
                )
                .bold()
            );

            for session in &sessions {
                let id_short = &session.id.to_string()[..8];
                let started = session.started_at.format("%Y-%m-%d %H:%M").to_string();
                let branch_history = db.get_session_branch_history(session.id)?;
                let branch_display = format_branch_history(&branch_history);
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
                    branch_display.yellow(),
                    dir
                );
            }
        }
    }

    Ok(())
}

/// Formats a branch history for display.
///
/// Joins branches with arrows. If there are more than 3 branches,
/// truncates to show: first -> second -> ... -> last
///
/// Returns "-" if the history is empty.
fn format_branch_history(branches: &[String]) -> String {
    match branches.len() {
        0 => "-".to_string(),
        1 => branches[0].clone(),
        2 | 3 => branches.join(" -> "),
        _ => {
            // More than 3 branches: show first -> second -> ... -> last
            format!(
                "{} -> {} -> ... -> {}",
                branches[0],
                branches[1],
                branches.last().unwrap()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_branch_history_empty() {
        let branches: Vec<String> = vec![];
        assert_eq!(format_branch_history(&branches), "-");
    }

    #[test]
    fn test_format_branch_history_single() {
        let branches = vec!["main".to_string()];
        assert_eq!(format_branch_history(&branches), "main");
    }

    #[test]
    fn test_format_branch_history_two() {
        let branches = vec!["main".to_string(), "feat/auth".to_string()];
        assert_eq!(format_branch_history(&branches), "main -> feat/auth");
    }

    #[test]
    fn test_format_branch_history_three() {
        let branches = vec![
            "main".to_string(),
            "feat/auth".to_string(),
            "main".to_string(),
        ];
        assert_eq!(
            format_branch_history(&branches),
            "main -> feat/auth -> main"
        );
    }

    #[test]
    fn test_format_branch_history_truncated() {
        let branches = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "feat/b".to_string(),
            "feat/c".to_string(),
            "main".to_string(),
        ];
        assert_eq!(
            format_branch_history(&branches),
            "main -> feat/a -> ... -> main"
        );
    }
}

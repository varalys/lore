//! Status command - show current Lore state.
//!
//! Displays an overview of the Lore database including the number
//! of imported sessions, discovered session files, and recent
//! session activity.

use anyhow::Result;
use colored::Colorize;

use crate::capture::watchers::claude_code;
use crate::storage::Database;

/// Executes the status command.
///
/// Shows database statistics, available session sources, and
/// recent sessions.
pub fn run() -> Result<()> {
    println!("{}", "Lore".bold().cyan());
    println!("{}", "Reasoning history for code".dimmed());
    println!();

    // Check Claude Code sessions
    let session_files = claude_code::find_session_files()?;
    println!(
        "{}",
        format!("Claude Code sessions found: {}", session_files.len()).green()
    );

    // Check database
    let db = Database::open_default()?;
    let session_count = db.session_count()?;
    let message_count = db.message_count()?;

    println!();
    println!("{}", "Database:".bold());
    println!("  Sessions imported: {session_count}");
    println!("  Messages stored:   {message_count}");

    if session_count == 0 && !session_files.is_empty() {
        println!();
        println!(
            "{}",
            "Hint: Run 'lore import' to import Claude Code sessions".yellow()
        );
    }

    // Show recent sessions if any
    let recent = db.list_sessions(5, None)?;
    if !recent.is_empty() {
        println!();
        println!("{}", "Recent sessions:".bold());
        for session in recent {
            let id_short = &session.id.to_string()[..8];
            let ago = chrono::Utc::now()
                .signed_duration_since(session.started_at)
                .num_hours();
            let ago_str = if ago < 1 {
                "just now".to_string()
            } else if ago < 24 {
                format!("{ago} hours ago")
            } else {
                format!("{} days ago", ago / 24)
            };

            let branch = session.git_branch.as_deref().unwrap_or("-");
            let dir = session
                .working_directory
                .split('/')
                .next_back()
                .unwrap_or(&session.working_directory);

            println!(
                "  {}  {:12}  {:10}  {}  {}",
                id_short.cyan(),
                ago_str.dimmed(),
                format!("{} msgs", session.message_count),
                branch.yellow(),
                dir
            );
        }
    }

    Ok(())
}

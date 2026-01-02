//! Delete command - permanently remove a session and its data.
//!
//! Deletes a session and all its associated messages and links from the
//! database. This operation is irreversible.

use std::io::{self, Write};

use anyhow::{bail, Result};
use colored::Colorize;

use crate::storage::Database;

/// Arguments for the delete command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore delete abc123             Delete session (prompts for confirmation)\n    \
    lore delete abc123 --force     Delete without confirmation")]
pub struct Args {
    /// Session ID prefix to delete
    #[arg(value_name = "SESSION")]
    #[arg(
        long_help = "The session ID prefix to delete. Must uniquely identify a\n\
        single session. Use 'lore sessions' to find session IDs."
    )]
    pub session: String,

    /// Skip the confirmation prompt
    #[arg(long)]
    #[arg(
        long_help = "Skip the confirmation prompt and proceed with deletion.\n\
        Use with caution as this operation cannot be undone."
    )]
    pub force: bool,
}

/// Executes the delete command.
///
/// Permanently removes a session and all its associated data (messages, links)
/// from the database.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Find session by prefix
    let all_sessions = db.list_sessions(1000, None)?;
    let matching: Vec<_> = all_sessions
        .iter()
        .filter(|s| s.id.to_string().starts_with(&args.session))
        .collect();

    if matching.is_empty() {
        if all_sessions.is_empty() {
            bail!(
                "No session found matching '{}'. No sessions in database. \
                 Run 'lore import' to import sessions first.",
                args.session
            );
        } else {
            bail!(
                "No session found matching '{}'. \
                 Run 'lore sessions' to list available sessions.",
                args.session
            );
        }
    }

    if matching.len() > 1 {
        println!("{}", "Multiple sessions match that prefix:".yellow());
        for s in &matching {
            let id_short = &s.id.to_string()[..8];
            println!(
                "  {} - {}",
                id_short.cyan(),
                s.started_at.format("%Y-%m-%d %H:%M")
            );
        }
        bail!(
            "Multiple sessions match '{}'. Please use a more specific prefix from the list above.",
            args.session
        );
    }

    let session = matching[0];
    let session_short = &session.id.to_string()[..8];

    // Get counts for what will be deleted
    let messages = db.get_messages(&session.id)?;
    let links = db.get_links_by_session(&session.id)?;

    // Show what will be deleted
    println!();
    println!("{} {}", "Session".bold(), session.id.to_string().cyan());
    println!("  {}  {}", "Tool:".dimmed(), session.tool);
    println!(
        "  {}  {}",
        "Started:".dimmed(),
        session.started_at.format("%Y-%m-%d %H:%M:%S")
    );
    println!("  {}  {}", "Directory:".dimmed(), session.working_directory);
    if let Some(ref branch) = session.git_branch {
        println!("  {}  {}", "Branch:".dimmed(), branch);
    }
    println!();
    println!(
        "{}",
        format!(
            "This will permanently delete {} messages and {} links.",
            messages.len(),
            links.len()
        )
        .yellow()
    );

    // Confirm unless --force
    if !args.force {
        print!("Delete session {}? [y/N] ", session_short.cyan());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("{}", "Cancelled".dimmed());
            return Ok(());
        }
    }

    // Delete the session
    let (messages_deleted, links_deleted) = db.delete_session(&session.id)?;

    println!(
        "{} session {} ({} messages, {} links)",
        "Deleted".green(),
        session_short.cyan(),
        messages_deleted,
        links_deleted
    );

    Ok(())
}

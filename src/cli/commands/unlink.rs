//! Unlink command - remove session-to-commit links.
//!
//! Removes associations between sessions and commits. Can unlink
//! a session from all commits or from a specific commit.

use std::io::{self, Write};

use anyhow::{bail, Result};
use colored::Colorize;

use crate::storage::Database;

/// Arguments for the unlink command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore unlink abc123                  Unlink from all commits (prompts)\n    \
    lore unlink abc123 -y               Unlink from all commits (no prompt)\n    \
    lore unlink abc123 --commit 1a2b    Unlink from specific commit")]
pub struct Args {
    /// Session ID prefix to unlink
    #[arg(value_name = "SESSION")]
    #[arg(
        long_help = "The session ID prefix to unlink. Must uniquely identify a\n\
        single session. Use 'lore sessions' to find session IDs."
    )]
    pub session: String,

    /// Specific commit to unlink from (removes all links if omitted)
    #[arg(long, value_name = "SHA")]
    #[arg(long_help = "If specified, only removes the link to this commit.\n\
        If omitted, removes all links for the session.")]
    pub commit: Option<String>,

    /// Skip the confirmation prompt
    #[arg(short = 'y', long)]
    #[arg(
        long_help = "Skip the confirmation prompt and proceed with unlinking.\n\
        Use with caution when removing all links from a session."
    )]
    pub yes: bool,
}

/// Executes the unlink command.
///
/// Removes links between a session and commits. If --commit is specified,
/// only removes the link to that specific commit. Otherwise, removes all
/// links for the session.
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

    // Get existing links for the session
    let links = db.get_links_by_session(&session.id)?;

    if links.is_empty() {
        println!(
            "{}",
            format!("Session {session_short} has no links to remove").dimmed()
        );
        return Ok(());
    }

    // Determine which links to remove
    if let Some(ref commit_sha) = args.commit {
        // Remove only the link to the specific commit
        let matching_links: Vec<_> = links
            .iter()
            .filter(|l| {
                l.commit_sha
                    .as_ref()
                    .is_some_and(|sha| sha.starts_with(commit_sha))
            })
            .collect();

        if matching_links.is_empty() {
            bail!(
                "No link found between session {session_short} and commit {commit_sha}. \
                 Run 'lore show {session_short}' to see linked commits for this session."
            );
        }

        let link = matching_links[0];
        let link_sha = link.commit_sha.as_ref().map_or("unknown", |s| s.as_str());
        let short_sha = &link_sha[..8.min(link_sha.len())];

        // Confirm unless --yes
        if !args.yes {
            print!(
                "Unlink session {} from commit {}? [y/N] ",
                session_short.cyan(),
                short_sha.yellow()
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if !input.trim().eq_ignore_ascii_case("y") {
                println!("{}", "Cancelled".dimmed());
                return Ok(());
            }
        }

        // Delete the link
        let deleted = db.delete_link_by_session_and_commit(&session.id, commit_sha)?;

        if deleted {
            println!(
                "{} session {} from commit {}",
                "Unlinked".green(),
                session_short.cyan(),
                short_sha
            );
        } else {
            bail!("Failed to delete link");
        }
    } else {
        // Remove all links for the session
        let commit_count = links.len();

        // Confirm unless --yes
        if !args.yes {
            println!("This will unlink session {} from:", session_short.cyan());
            for link in &links {
                if let Some(ref sha) = link.commit_sha {
                    let short_sha = &sha[..8.min(sha.len())];
                    println!("  - commit {}", short_sha.yellow());
                }
            }
            print!("Continue? [y/N] ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if !input.trim().eq_ignore_ascii_case("y") {
                println!("{}", "Cancelled".dimmed());
                return Ok(());
            }
        }

        // Delete all links
        let deleted_count = db.delete_links_by_session(&session.id)?;

        if deleted_count == 1 {
            let sha = links[0]
                .commit_sha
                .as_ref()
                .map_or("unknown", |s| s.as_str());
            let short_sha = &sha[..8.min(sha.len())];
            println!(
                "{} session {} from commit {}",
                "Unlinked".green(),
                session_short.cyan(),
                short_sha
            );
        } else {
            println!(
                "{} session {} from {} commits",
                "Unlinked".green(),
                session_short.cyan(),
                commit_count
            );
        }
    }

    Ok(())
}

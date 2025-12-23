//! Import command - import sessions from AI coding tools.
//!
//! Discovers and imports session files from Claude Code into the
//! Lore database. Tracks which files have been imported to avoid
//! duplicates on subsequent runs.

use anyhow::Result;
use colored::Colorize;

use crate::capture::watchers::claude_code;
use crate::storage::Database;

/// Arguments for the import command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore import              Import new sessions\n    \
    lore import --dry-run    Preview what would be imported\n    \
    lore import --force      Re-import all sessions")]
pub struct Args {
    /// Force re-import of already imported sessions
    #[arg(long)]
    #[arg(long_help = "By default, Lore tracks which session files have been imported\n\
        and skips them on subsequent runs. Use this flag to re-import\n\
        all sessions, which may update existing records.")]
    pub force: bool,

    /// Preview what would be imported without making changes
    #[arg(long)]
    #[arg(long_help = "Shows what sessions would be imported without actually\n\
        modifying the database. Useful for verifying before import.")]
    pub dry_run: bool,
}

/// Executes the import command.
///
/// Scans for Claude Code session files, parses them, and stores
/// sessions and messages in the database.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    println!("{}", "Scanning for Claude Code sessions...".dimmed());

    let session_files = claude_code::find_session_files()?;

    if session_files.is_empty() {
        println!();
        println!("{}", "No Claude Code sessions found.".yellow());
        println!(
            "{}",
            "Sessions are stored in ~/.claude/projects/".dimmed()
        );
        return Ok(());
    }

    println!(
        "Found {} session files",
        session_files.len().to_string().green()
    );
    println!();

    let mut imported = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for path in session_files {
        let path_str = path.to_string_lossy();

        // Check if already imported
        if !args.force && db.session_exists_by_source(&path_str)? {
            skipped += 1;
            tracing::debug!("Skipping already imported: {}", path_str);
            continue;
        }

        // Parse the session
        match claude_code::parse_session_file(&path) {
            Ok(parsed) => {
                if parsed.messages.is_empty() {
                    tracing::debug!("Skipping empty session: {}", path_str);
                    skipped += 1;
                    continue;
                }

                let (session, messages) = parsed.to_storage_models();

                if args.dry_run {
                    let dir = session
                        .working_directory
                        .split('/')
                        .next_back()
                        .unwrap_or(&session.working_directory);
                    println!(
                        "  {} {} ({} messages, {})",
                        "Would import:".dimmed(),
                        &session.id.to_string()[..8].cyan(),
                        messages.len(),
                        dir
                    );
                    imported += 1;
                } else {
                    // Store session
                    db.insert_session(&session)?;

                    // Store messages
                    for msg in &messages {
                        db.insert_message(msg)?;
                    }

                    let dir = session
                        .working_directory
                        .split('/')
                        .next_back()
                        .unwrap_or(&session.working_directory);

                    println!(
                        "  {} {} ({} messages, {})",
                        "Imported:".green(),
                        &session.id.to_string()[..8].cyan(),
                        messages.len(),
                        dir
                    );

                    imported += 1;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}", path_str, e);
                errors += 1;
            }
        }
    }

    println!();
    if args.dry_run {
        println!(
            "{}",
            format!(
                "Dry run: would import {imported}, skip {skipped}, {errors} errors"
            )
            .bold()
        );
    } else {
        println!(
            "{}",
            format!(
                "Imported {imported}, skipped {skipped}, {errors} errors"
            )
            .bold()
        );

        if imported > 0 {
            println!();
            println!("{}", "Run 'lore sessions' to see imported sessions".dimmed());
        }
    }

    Ok(())
}

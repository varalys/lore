//! Import command - import sessions from AI coding tools.
//!
//! Discovers and imports session files from multiple AI coding tools into the
//! Lore database. Tracks which files have been imported to avoid duplicates
//! on subsequent runs.
//!
//! Supported tools:
//! - Aider (markdown chat history files)
//! - Claude Code (JSONL files)
//! - Cline (VS Code extension storage)
//! - Codex CLI (JSONL files)
//! - Continue.dev (JSON session files)
//! - Cursor IDE (SQLite databases, experimental)
//! - Gemini CLI (JSON files)

use anyhow::Result;
use colored::Colorize;

use crate::capture::watchers::default_registry;
use crate::storage::Database;

/// Arguments for the import command.
#[derive(clap::Args)]
#[command(
    about = "Import sessions from AI coding tools",
    long_about = "Import sessions from AI coding tools.\n\n\
        Supported tools:\n  \
        - Aider (markdown chat history files)\n  \
        - Claude Code (JSONL files in ~/.claude/projects/)\n  \
        - Cline (VS Code extension storage)\n  \
        - Codex CLI (JSONL files in ~/.codex/sessions/)\n  \
        - Continue.dev (JSON files in ~/.continue/sessions/)\n  \
        - Cursor IDE (SQLite databases, experimental)\n  \
        - Gemini CLI (JSON files in ~/.gemini/tmp/)",
    after_help = "EXAMPLES:\n    \
        lore import              Import new sessions from all tools\n    \
        lore import --dry-run    Preview what would be imported\n    \
        lore import --force      Re-import all sessions"
)]
pub struct Args {
    /// Force re-import of already imported sessions
    #[arg(long)]
    #[arg(
        long_help = "By default, Lore tracks which session files have been imported\n\
        and skips them on subsequent runs. Use this flag to re-import\n\
        all sessions, which may update existing records."
    )]
    pub force: bool,

    /// Preview what would be imported without making changes
    #[arg(long)]
    #[arg(long_help = "Shows what sessions would be imported without actually\n\
        modifying the database. Useful for verifying before import.")]
    pub dry_run: bool,
}

/// Executes the import command.
///
/// Scans for session files from all available AI coding tools, parses them,
/// and stores sessions and messages in the database.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;
    let registry = default_registry();

    let mut total_imported = 0;
    let mut total_skipped = 0;
    let mut total_errors = 0;
    let mut any_watcher_available = false;

    for watcher in registry.all_watchers() {
        let info = watcher.info();

        // Skip watchers that are not available
        if !watcher.is_available() {
            tracing::debug!("Skipping unavailable watcher: {}", info.name);
            continue;
        }

        any_watcher_available = true;
        println!("{}", format!("Scanning {}...", info.name).dimmed());

        // Find source files for this watcher
        let sources = match watcher.find_sources() {
            Ok(sources) => sources,
            Err(e) => {
                tracing::warn!("Failed to find sources for {}: {}", info.name, e);
                println!(
                    "  {}",
                    format!("Error finding sources: {e}").red()
                );
                total_errors += 1;
                continue;
            }
        };

        if sources.is_empty() {
            println!("  {}", "No sessions found".dimmed());
            continue;
        }

        println!(
            "  Found {} source files",
            sources.len().to_string().green()
        );

        let mut watcher_imported = 0;
        let mut watcher_skipped = 0;
        let mut watcher_errors = 0;

        for path in sources {
            let path_str = path.to_string_lossy();

            // Check if already imported
            if !args.force && db.session_exists_by_source(&path_str)? {
                watcher_skipped += 1;
                tracing::debug!("Skipping already imported: {}", path_str);
                continue;
            }

            // Parse the source file
            match watcher.parse_source(&path) {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        tracing::debug!("No sessions in source: {}", path_str);
                        watcher_skipped += 1;
                        continue;
                    }

                    for (session, messages) in sessions {
                        if messages.is_empty() {
                            tracing::debug!("Skipping empty session: {}", session.id);
                            watcher_skipped += 1;
                            continue;
                        }

                        if args.dry_run {
                            let dir = session
                                .working_directory
                                .split('/')
                                .next_back()
                                .unwrap_or(&session.working_directory);
                            println!(
                                "    {} {} ({} messages, {})",
                                "Would import:".dimmed(),
                                &session.id.to_string()[..8].cyan(),
                                messages.len(),
                                dir
                            );
                            watcher_imported += 1;
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
                                "    {} {} ({} messages, {})",
                                "Imported:".green(),
                                &session.id.to_string()[..8].cyan(),
                                messages.len(),
                                dir
                            );

                            watcher_imported += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path_str, e);
                    watcher_errors += 1;
                }
            }
        }

        total_imported += watcher_imported;
        total_skipped += watcher_skipped;
        total_errors += watcher_errors;
    }

    println!();

    if !any_watcher_available {
        println!("{}", "No AI coding tools detected.".yellow());
        println!(
            "{}",
            "Install and use one of the supported tools to create sessions.".dimmed()
        );
        return Ok(());
    }

    if args.dry_run {
        println!(
            "{}",
            format!(
                "Dry run: would import {total_imported}, skip {total_skipped}, {total_errors} errors"
            )
            .bold()
        );
    } else {
        println!(
            "{}",
            format!(
                "Imported {total_imported}, skipped {total_skipped}, {total_errors} errors"
            )
            .bold()
        );

        if total_imported > 0 {
            println!();
            println!(
                "{}",
                "Run 'lore sessions' to see imported sessions".dimmed()
            );
        }
    }

    Ok(())
}

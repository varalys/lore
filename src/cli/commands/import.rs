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
use crate::config::Config;
use crate::storage::Database;

/// Arguments for the import command.
#[derive(clap::Args)]
#[command(
    about = "Import sessions from AI coding tools",
    long_about = "Import sessions from AI coding tools.\n\n\
        Supported tools:\n  \
        - Aider (markdown chat history files)\n  \
        - Amp (JSON files in ~/.local/share/amp/threads/)\n  \
        - Claude Code (JSONL files in ~/.claude/projects/)\n  \
        - Cline (VS Code extension storage)\n  \
        - Codex CLI (JSONL files in ~/.codex/sessions/)\n  \
        - Continue.dev (JSON files in ~/.continue/sessions/)\n  \
        - Gemini CLI (JSON files in ~/.gemini/tmp/)\n  \
        - OpenCode (JSON files in ~/.local/share/opencode/storage/)\n  \
        - Roo Code (JSON in VS Code extension storage)",
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
/// Scans for session files from enabled AI coding tools, parses them,
/// and stores sessions and messages in the database. Uses the configuration
/// to determine which watchers are enabled.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;
    let config = Config::load()?;
    let registry = default_registry();

    // Get enabled watchers from config
    let watchers = registry.enabled_watchers(&config.watchers);

    let mut total_imported = 0;
    let mut total_skipped = 0;
    let mut total_errors = 0;
    let mut tools_imported_from = 0;

    if watchers.is_empty() {
        println!("{}", "No enabled watchers found.".yellow());
        println!();
        println!("Check your configuration with: {}", "lore config".cyan());
        println!("Run {} to detect and enable watchers.", "lore init".cyan());
        return Ok(());
    }

    for watcher in &watchers {
        let info = watcher.info();
        println!("{}", format!("Importing from {}...", info.name).dimmed());

        // Find source files for this watcher
        let sources = match watcher.find_sources() {
            Ok(sources) => sources,
            Err(e) => {
                tracing::warn!("Failed to find sources for {}: {}", info.name, e);
                println!("  {}", format!("Error finding sources: {e}").red());
                total_errors += 1;
                continue;
            }
        };

        if sources.is_empty() {
            println!("  {}", "No sessions found".dimmed());
            continue;
        }

        println!("  Found {} source files", sources.len().to_string().green());

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

        if watcher_imported > 0 {
            tools_imported_from += 1;
        }
        total_imported += watcher_imported;
        total_skipped += watcher_skipped;
        total_errors += watcher_errors;
    }

    println!();

    if args.dry_run {
        println!(
            "{}",
            format!(
                "Dry run: would import {total_imported} sessions from {tools_imported_from} tools"
            )
            .bold()
        );
        if total_skipped > 0 || total_errors > 0 {
            println!("  ({total_skipped} skipped, {total_errors} errors)");
        }
    } else {
        println!(
            "{}",
            format!("Imported {total_imported} sessions from {tools_imported_from} tools").bold()
        );
        if total_skipped > 0 || total_errors > 0 {
            println!("  ({total_skipped} skipped, {total_errors} errors)");
        }

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

/// Import statistics returned by the import operation.
#[derive(Debug, Default)]
pub struct ImportStats {
    /// Number of sessions imported.
    pub imported: usize,
    /// Number of sessions skipped.
    pub skipped: usize,
    /// Number of errors encountered.
    pub errors: usize,
    /// Number of tools that had sessions imported.
    pub tools_count: usize,
}

/// Runs the import operation and returns statistics.
///
/// This is a lower-level function that can be called from other commands
/// (like init) without printing output. The caller is responsible for
/// displaying results.
pub fn run_import(force: bool, dry_run: bool) -> Result<ImportStats> {
    let db = Database::open_default()?;
    let config = Config::load()?;
    let registry = default_registry();

    let watchers = registry.enabled_watchers(&config.watchers);

    let mut stats = ImportStats::default();

    for watcher in &watchers {
        let info = watcher.info();
        println!("{}", format!("Importing from {}...", info.name).dimmed());

        let sources = match watcher.find_sources() {
            Ok(sources) => sources,
            Err(e) => {
                tracing::warn!("Failed to find sources for {}: {}", info.name, e);
                println!("  {}", format!("Error finding sources: {e}").red());
                stats.errors += 1;
                continue;
            }
        };

        if sources.is_empty() {
            println!("  {}", "No sessions found".dimmed());
            continue;
        }

        println!("  Found {} source files", sources.len().to_string().green());

        let mut watcher_imported = 0;

        for path in sources {
            let path_str = path.to_string_lossy();

            if !force && db.session_exists_by_source(&path_str)? {
                stats.skipped += 1;
                tracing::debug!("Skipping already imported: {}", path_str);
                continue;
            }

            match watcher.parse_source(&path) {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        tracing::debug!("No sessions in source: {}", path_str);
                        stats.skipped += 1;
                        continue;
                    }

                    for (session, messages) in sessions {
                        if messages.is_empty() {
                            tracing::debug!("Skipping empty session: {}", session.id);
                            stats.skipped += 1;
                            continue;
                        }

                        if dry_run {
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
                            db.insert_session(&session)?;

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
                    stats.errors += 1;
                }
            }
        }

        if watcher_imported > 0 {
            stats.tools_count += 1;
        }
        stats.imported += watcher_imported;
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_stats_default() {
        let stats = ImportStats::default();
        assert_eq!(stats.imported, 0);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.tools_count, 0);
    }

    #[test]
    fn test_import_stats_fields_are_accessible() {
        let stats = ImportStats {
            imported: 10,
            skipped: 5,
            errors: 2,
            tools_count: 3,
        };
        assert_eq!(stats.imported, 10);
        assert_eq!(stats.skipped, 5);
        assert_eq!(stats.errors, 2);
        assert_eq!(stats.tools_count, 3);
    }
}

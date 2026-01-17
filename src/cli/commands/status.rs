//! Status command - show current Lore state.
//!
//! Displays an overview of the Lore database including the number
//! of imported sessions, discovered session files, recent session
//! activity, daemon status, and links to the current commit.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::capture::watchers::{default_registry, WatcherRegistry};
use crate::cli::OutputFormat;
use crate::cloud::CredentialsStore;
use crate::config::Config;
use crate::daemon::{send_command_sync, DaemonCommand, DaemonResponse, DaemonState};
use crate::git;
use crate::storage::Database;

/// The current CLI version.
const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Arguments for the status command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore status               Show status overview\n    \
    lore status --format json Output as JSON")]
pub struct Args {
    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for status.
#[derive(Serialize)]
struct StatusOutput {
    daemon: DaemonStatus,
    watchers: Vec<WatcherStatus>,
    database: DatabaseStats,
    current_commit: Option<CurrentCommitInfo>,
    recent_sessions: Vec<RecentSessionInfo>,
}

/// Daemon status for JSON output.
#[derive(Serialize)]
struct DaemonStatus {
    running: bool,
    message: String,
}

/// Watcher status for JSON output.
#[derive(Serialize)]
struct WatcherStatus {
    name: String,
    available: bool,
    enabled: bool,
    session_files: Option<usize>,
}

/// Database statistics for JSON output.
#[derive(Serialize)]
struct DatabaseStats {
    sessions: i32,
    messages: i32,
    links: i32,
    size_bytes: Option<u64>,
}

/// Current commit info for JSON output.
#[derive(Serialize)]
struct CurrentCommitInfo {
    sha: String,
    linked_sessions: usize,
    session_ids: Vec<String>,
}

/// Recent session info for JSON output.
#[derive(Serialize)]
struct RecentSessionInfo {
    id: String,
    started_at: String,
    message_count: i32,
    branch: Option<String>,
    directory: String,
}

/// Executes the status command.
///
/// Shows database statistics, available session sources, daemon status,
/// watcher availability, current commit links, and recent sessions.
pub fn run(args: Args) -> Result<()> {
    let registry = default_registry();
    let db = Database::open_default()?;
    let config = Config::load()?;

    match args.format {
        OutputFormat::Json => {
            run_json(&db, &registry, &config)?;
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            run_text(&db, &registry, &config)?;
        }
    }

    Ok(())
}

/// Runs the status command with JSON output.
fn run_json(db: &Database, registry: &WatcherRegistry, config: &Config) -> Result<()> {
    // Collect watcher status
    let mut watchers = Vec::new();
    for watcher in registry.all_watchers() {
        let info = watcher.info();
        let is_enabled = config.watchers.iter().any(|w| w == info.name);
        let session_files = if watcher.is_available() {
            watcher.find_sources().ok().map(|s| s.len())
        } else {
            None
        };
        watchers.push(WatcherStatus {
            name: info.name.to_string(),
            available: watcher.is_available(),
            enabled: is_enabled,
            session_files,
        });
    }
    // Add copilot placeholder
    watchers.push(WatcherStatus {
        name: "copilot".to_string(),
        available: false,
        enabled: false,
        session_files: None,
    });

    // Database stats
    let session_count = db.session_count()?;
    let message_count = db.message_count()?;
    let link_count = db.link_count()?;
    let size_bytes = db
        .db_path()
        .and_then(|p| std::fs::metadata(&p).ok())
        .map(|m| m.len());

    // Current commit
    let current_commit = get_current_commit_info(db)?;

    // Recent sessions
    let recent = db.list_sessions(5, None)?;
    let recent_sessions: Vec<RecentSessionInfo> = recent
        .into_iter()
        .map(|s| RecentSessionInfo {
            id: s.id.to_string(),
            started_at: s.started_at.to_rfc3339(),
            message_count: s.message_count,
            branch: s.git_branch,
            directory: s.working_directory,
        })
        .collect();

    // Check daemon status
    let daemon_status = match DaemonState::new() {
        Ok(state) => {
            if state.is_running() {
                let pid = state.get_pid().unwrap_or(0);
                DaemonStatus {
                    running: true,
                    message: format!("running (PID {})", pid),
                }
            } else {
                DaemonStatus {
                    running: false,
                    message: "not running".to_string(),
                }
            }
        }
        Err(_) => DaemonStatus {
            running: false,
            message: "not running".to_string(),
        },
    };

    let output = StatusOutput {
        daemon: daemon_status,
        watchers,
        database: DatabaseStats {
            sessions: session_count,
            messages: message_count,
            links: link_count,
            size_bytes,
        },
        current_commit,
        recent_sessions,
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{json}");

    Ok(())
}

/// Gets current commit info for JSON output.
fn get_current_commit_info(db: &Database) -> Result<Option<CurrentCommitInfo>> {
    let cwd = std::env::current_dir()?;
    let repo_info = match git::repo_info(&cwd) {
        Ok(info) => info,
        Err(_) => return Ok(None),
    };

    let commit_sha = match repo_info.commit_sha {
        Some(sha) => sha,
        None => return Ok(None),
    };

    let links = db.get_links_by_commit(&commit_sha)?;
    let session_ids: Vec<String> = links.iter().map(|l| l.session_id.to_string()).collect();

    Ok(Some(CurrentCommitInfo {
        sha: commit_sha,
        linked_sessions: links.len(),
        session_ids,
    }))
}

/// Runs the status command with text output.
fn run_text(db: &Database, registry: &WatcherRegistry, config: &Config) -> Result<()> {
    println!("{}", "Lore".bold().cyan());
    println!("{}", "Reasoning history for code".dimmed());
    println!();

    // Daemon status placeholder
    print_daemon_status();

    // Watchers section
    print_watchers_status(registry, config);

    // Database statistics
    print_database_stats(db)?;

    // Cloud account status
    let is_logged_in = print_cloud_status(db, config);

    // Current commit section (if in a git repo)
    print_current_commit_links(db)?;

    // Show hint if sessions exist but are not imported
    let session_count = db.session_count()?;
    let has_available_sources = registry
        .available_watchers()
        .iter()
        .any(|w| w.find_sources().map(|s| !s.is_empty()).unwrap_or(false));

    if session_count == 0 && has_available_sources {
        println!();
        println!(
            "{}",
            "Hint: Run 'lore import' to import sessions from available sources".yellow()
        );
    }

    // Show recent sessions if any
    print_recent_sessions(db)?;

    // Show login tip only if not logged in
    if !is_logged_in {
        print_login_tip();
    }

    Ok(())
}

/// Prints the daemon status section.
///
/// Shows whether the daemon is currently running and its PID if available.
/// Also checks if the daemon version matches the CLI version and warns if not.
fn print_daemon_status() {
    println!("{}", "Daemon:".bold());

    match DaemonState::new() {
        Ok(state) => {
            if state.is_running() {
                let pid = state.get_pid().unwrap_or(0);

                // Try to get daemon version via IPC
                match send_command_sync(&state.socket_path, DaemonCommand::Status) {
                    Ok(DaemonResponse::Status { version, .. }) => {
                        if version != CLI_VERSION {
                            println!(
                                "  {} (PID {}) {} {}",
                                "running".green(),
                                pid,
                                format!("v{}", version).dimmed(),
                                "(restart recommended)".yellow()
                            );
                            println!(
                                "  {}",
                                format!(
                                    "Daemon is v{}, CLI is v{}. Run: lore daemon stop && lore daemon start",
                                    version, CLI_VERSION
                                )
                                .yellow()
                            );
                        } else {
                            println!("  {} (PID {})", "running".green(), pid);
                        }
                    }
                    _ => {
                        // Couldn't get version, just show running status
                        println!("  {} (PID {})", "running".green(), pid);
                    }
                }
            } else {
                println!("  {}", "not running".yellow());
            }
        }
        Err(_) => {
            println!("  {}", "not running".yellow());
        }
    }

    println!();
}

/// Prints the watchers availability section.
///
/// Shows which session watchers are enabled, available, and how many session files
/// each has discovered. Distinguishes between enabled (in config) and available
/// (tool installed).
fn print_watchers_status(registry: &WatcherRegistry, config: &Config) {
    println!("{}", "Watchers:".bold());

    for watcher in registry.all_watchers() {
        let info = watcher.info();
        let name = info.name;
        let is_enabled = config.watchers.iter().any(|w| w == name);

        if watcher.is_available() {
            let status_str = if is_enabled {
                "enabled".green().to_string()
            } else {
                "available (not enabled)".yellow().to_string()
            };

            match watcher.find_sources() {
                Ok(sources) if !sources.is_empty() => {
                    println!(
                        "  {}: {} ({} session files)",
                        name.cyan(),
                        status_str,
                        sources.len()
                    );
                }
                Ok(_) => {
                    println!("  {}: {} (no sessions found)", name.cyan(), status_str);
                }
                Err(_) => {
                    println!("  {}: {} (error reading sources)", name.cyan(), status_str);
                }
            }
        } else {
            println!("  {}: {}", name.cyan(), "not available".dimmed());
        }
    }

    // Note about future watchers
    println!("  {}: {}", "copilot".cyan(), "not available".dimmed());

    println!();
}

/// Prints the cloud account status section.
///
/// Shows whether the user is logged in, their account info, and last sync time.
/// Returns true if logged in, false otherwise.
fn print_cloud_status(db: &Database, config: &Config) -> bool {
    let store = CredentialsStore::with_keychain(config.use_keychain);

    match store.load() {
        Ok(Some(creds)) => {
            println!();
            println!("{}", "Cloud:".bold());
            println!(
                "  Logged in as {} ({} plan)",
                creds.email.cyan(),
                creds.plan
            );

            // Show last sync time
            match db.last_sync_time() {
                Ok(Some(last_sync)) => {
                    println!("  Last sync: {}", format_relative_time(last_sync));
                }
                Ok(None) => {
                    println!("  Last sync: {}", "never".dimmed());
                }
                Err(_) => {
                    // Silently skip if we cannot read sync time
                }
            }
            true
        }
        _ => false,
    }
}

/// Prints enhanced database statistics.
///
/// Shows total sessions, messages, links, and database file size.
fn print_database_stats(db: &Database) -> Result<()> {
    let session_count = db.session_count()?;
    let message_count = db.message_count()?;
    let link_count = db.link_count()?;

    println!("{}", "Database:".bold());
    println!("  Sessions: {session_count}");
    println!("  Messages: {message_count}");
    println!("  Links:    {link_count}");

    // Try to get database file size
    if let Some(db_path) = db.db_path() {
        if let Ok(metadata) = std::fs::metadata(&db_path) {
            let size_bytes = metadata.len();
            let size_str = format_file_size(size_bytes);
            println!("  Size:     {size_str}");
        }
    }

    Ok(())
}

/// Formats a file size in bytes to a human-readable string.
fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

/// Formats a timestamp as relative time (e.g., "2 hours ago", "3 days ago").
fn format_relative_time(time: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(time);

    let hours = duration.num_hours();
    if hours < 1 {
        let minutes = duration.num_minutes();
        if minutes < 1 {
            "just now".to_string()
        } else if minutes == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{} minutes ago", minutes)
        }
    } else if hours == 1 {
        "1 hour ago".to_string()
    } else if hours < 24 {
        format!("{} hours ago", hours)
    } else {
        let days = duration.num_days();
        if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{} days ago", days)
        }
    }
}

/// Prints information about sessions linked to the current HEAD commit.
///
/// If not in a git repository, this section is silently skipped.
fn print_current_commit_links(db: &Database) -> Result<()> {
    // Try to get current git repo info
    let cwd = std::env::current_dir()?;
    let repo_info = match git::repo_info(&cwd) {
        Ok(info) => info,
        Err(_) => {
            // Not in a git repo, skip this section
            return Ok(());
        }
    };

    let commit_sha = match repo_info.commit_sha {
        Some(sha) => sha,
        None => {
            // No commits yet
            println!();
            println!("{}", "Current commit:".bold());
            println!("  {}", "No commits yet".dimmed());
            return Ok(());
        }
    };

    let short_sha = &commit_sha[..7.min(commit_sha.len())];

    println!();
    println!("{} ({}):", "Current commit".bold(), short_sha.yellow());

    // Get links for this commit
    let links = db.get_links_by_commit(&commit_sha)?;

    if links.is_empty() {
        println!("  {}", "No linked sessions".dimmed());
    } else {
        println!("  Linked sessions: {}", links.len());

        for link in &links {
            // Get session details
            if let Ok(Some(session)) = db.get_session(&link.session_id) {
                let id_short = &session.id.to_string()[..8];
                println!(
                    "  - {} ({} messages)",
                    id_short.cyan(),
                    session.message_count
                );
            } else {
                let id_short = &link.session_id.to_string()[..8];
                println!("  - {} (session not found)", id_short.cyan());
            }
        }
    }

    Ok(())
}

/// Prints the recent sessions section.
fn print_recent_sessions(db: &Database) -> Result<()> {
    let recent = db.list_sessions(5, None)?;
    if recent.is_empty() {
        return Ok(());
    }

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

    Ok(())
}

/// Prints a tip about logging in.
///
/// This helps users discover the cloud sync feature.
fn print_login_tip() {
    println!();
    println!(
        "{}",
        "Tip: Run 'lore login' to sync sessions across machines".dimmed()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_size_bytes() {
        assert_eq!(format_file_size(0), "0 bytes");
        assert_eq!(format_file_size(512), "512 bytes");
        assert_eq!(format_file_size(1023), "1023 bytes");
    }

    #[test]
    fn test_format_file_size_kilobytes() {
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(10240), "10.0 KB");
    }

    #[test]
    fn test_format_file_size_megabytes() {
        assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 5), "5.0 MB");
        assert_eq!(format_file_size(1024 * 1024 + 512 * 1024), "1.5 MB");
    }

    #[test]
    fn test_format_file_size_gigabytes() {
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_file_size(1024 * 1024 * 1024 * 2), "2.0 GB");
    }

    #[test]
    fn test_format_relative_time_just_now() {
        let now = chrono::Utc::now();
        assert_eq!(format_relative_time(now), "just now");

        let seconds_ago = now - chrono::Duration::seconds(30);
        assert_eq!(format_relative_time(seconds_ago), "just now");
    }

    #[test]
    fn test_format_relative_time_minutes() {
        let now = chrono::Utc::now();

        let one_min = now - chrono::Duration::minutes(1);
        assert_eq!(format_relative_time(one_min), "1 minute ago");

        let five_mins = now - chrono::Duration::minutes(5);
        assert_eq!(format_relative_time(five_mins), "5 minutes ago");

        let fifty_nine_mins = now - chrono::Duration::minutes(59);
        assert_eq!(format_relative_time(fifty_nine_mins), "59 minutes ago");
    }

    #[test]
    fn test_format_relative_time_hours() {
        let now = chrono::Utc::now();

        let one_hour = now - chrono::Duration::hours(1);
        assert_eq!(format_relative_time(one_hour), "1 hour ago");

        let two_hours = now - chrono::Duration::hours(2);
        assert_eq!(format_relative_time(two_hours), "2 hours ago");

        let twenty_three_hours = now - chrono::Duration::hours(23);
        assert_eq!(format_relative_time(twenty_three_hours), "23 hours ago");
    }

    #[test]
    fn test_format_relative_time_days() {
        let now = chrono::Utc::now();

        let one_day = now - chrono::Duration::days(1);
        assert_eq!(format_relative_time(one_day), "1 day ago");

        let three_days = now - chrono::Duration::days(3);
        assert_eq!(format_relative_time(three_days), "3 days ago");

        let thirty_days = now - chrono::Duration::days(30);
        assert_eq!(format_relative_time(thirty_days), "30 days ago");
    }
}

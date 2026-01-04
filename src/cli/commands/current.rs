//! Current command - show the active session for the current directory.
//!
//! Reports the session ID that is currently active for the working directory.
//! Queries the daemon if running, otherwise falls back to checking the database
//! for the most recent session.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::daemon::{send_command_sync, DaemonCommand, DaemonResponse, DaemonState};
use crate::storage::Database;

/// Arguments for the current command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore current              Show active session for cwd\n    \
    lore current --json       Output as JSON")]
pub struct Args {
    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for the current command.
#[derive(Serialize)]
struct CurrentOutput {
    session_id: Option<String>,
    working_directory: String,
    source: String,
}

/// Executes the current command.
///
/// Shows the active session ID for the current working directory.
/// Tries to query the daemon first, falls back to database lookup.
pub fn run(args: Args) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let working_dir = cwd.to_string_lossy().to_string();

    // Try to get the current session from the daemon first
    let (session_id, source) = match try_daemon_query(&working_dir) {
        Some(id) => (id, "daemon"),
        None => {
            // Fall back to database lookup
            let db = Database::open_default()?;
            let session = db.get_most_recent_session_for_directory(&working_dir)?;
            (session.map(|s| s.id.to_string()), "database")
        }
    };

    match args.format {
        OutputFormat::Json => {
            let output = CurrentOutput {
                session_id: session_id.clone(),
                working_directory: working_dir,
                source: source.to_string(),
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            if let Some(id) = session_id {
                let short_id = &id[..8.min(id.len())];
                println!("{}", short_id.cyan());
            } else {
                println!("{}", "No active session".dimmed());
            }
        }
    }

    Ok(())
}

/// Tries to query the daemon for the current session.
///
/// Returns Some(session_id) if the daemon is running and responds,
/// None if the daemon is not running or the query fails.
fn try_daemon_query(working_dir: &str) -> Option<Option<String>> {
    let state = DaemonState::new().ok()?;

    if !state.is_running() {
        return None;
    }

    let command = DaemonCommand::GetCurrentSession {
        working_directory: working_dir.to_string(),
    };

    match send_command_sync(&state.socket_path, command) {
        Ok(DaemonResponse::CurrentSession { session_id }) => Some(session_id),
        Ok(DaemonResponse::Error { message }) => {
            tracing::debug!("Daemon error: {}", message);
            None
        }
        Ok(_) => None,
        Err(e) => {
            tracing::debug!("Failed to query daemon: {}", e);
            None
        }
    }
}

//! Annotate command - add a bookmark or note to a session.
//!
//! Adds an annotation (bookmark or note) to the current active session.
//! Annotations help mark important moments in a session for later reference.

use anyhow::{bail, Result};
use chrono::Utc;
use colored::Colorize;
use uuid::Uuid;

use crate::daemon::{send_command_sync, DaemonCommand, DaemonResponse, DaemonState};
use crate::storage::{Annotation, Database};

/// Arguments for the annotate command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore annotate \"Fixed the auth bug\"     Add note to current session\n    \
    lore annotate \"Checkpoint\" --session abc123  Add to specific session")]
pub struct Args {
    /// The annotation content (note or bookmark text)
    #[arg(value_name = "NOTE")]
    pub note: String,

    /// Session ID prefix to annotate (uses current session if not specified)
    #[arg(long, short, value_name = "ID")]
    pub session: Option<String>,
}

/// Executes the annotate command.
///
/// Adds an annotation to the current or specified session.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Determine which session to annotate
    let session_id = match args.session {
        Some(id_prefix) => find_session_by_prefix(&db, &id_prefix)?,
        None => get_current_session(&db)?,
    };

    // Create the annotation
    let annotation = Annotation {
        id: Uuid::new_v4(),
        session_id,
        content: args.note,
        created_at: Utc::now(),
    };

    db.insert_annotation(&annotation)?;

    let short_id = &session_id.to_string()[..8];
    println!(
        "{}",
        format!("Annotation added to session {}", short_id.cyan()).green()
    );

    Ok(())
}

/// Finds a session by ID prefix.
///
/// Searches for sessions matching the given ID prefix and returns the full session ID.
fn find_session_by_prefix(db: &Database, id_prefix: &str) -> Result<Uuid> {
    let sessions = db.list_sessions(100, None)?;
    let matching: Vec<_> = sessions
        .iter()
        .filter(|s| s.id.to_string().starts_with(id_prefix))
        .collect();

    match matching.len() {
        0 => bail!("No session found matching '{id_prefix}'"),
        1 => Ok(matching[0].id),
        _ => {
            let ids: Vec<String> = matching.iter().map(|s| s.id.to_string()[..8].to_string()).collect();
            bail!(
                "Ambiguous session prefix '{}'. Matches: {}",
                id_prefix,
                ids.join(", ")
            )
        }
    }
}

/// Gets the current session for the working directory.
///
/// Tries the daemon first, then falls back to the database.
fn get_current_session(db: &Database) -> Result<Uuid> {
    let cwd = std::env::current_dir()?;
    let working_dir = cwd.to_string_lossy().to_string();

    // Try daemon first
    if let Some(session_id) = try_daemon_query(&working_dir) {
        return Uuid::parse_str(&session_id)
            .map_err(|e| anyhow::anyhow!("Invalid session ID from daemon: {e}"));
    }

    // Fall back to database
    let session = db.get_most_recent_session_for_directory(&working_dir)?;
    match session {
        Some(s) => Ok(s.id),
        None => bail!(
            "No active session found for this directory. \
             Use --session to specify a session ID, or run 'lore import' to import sessions."
        ),
    }
}

/// Tries to query the daemon for the current session.
fn try_daemon_query(working_dir: &str) -> Option<String> {
    let state = DaemonState::new().ok()?;

    if !state.is_running() {
        return None;
    }

    let command = DaemonCommand::GetCurrentSession {
        working_directory: working_dir.to_string(),
    };

    match send_command_sync(&state.socket_path, command) {
        Ok(DaemonResponse::CurrentSession { session_id }) => session_id,
        _ => None,
    }
}

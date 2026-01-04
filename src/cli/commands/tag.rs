//! Tag command - add or remove tags from sessions.
//!
//! Tags provide a way to categorize and organize sessions. Each session
//! can have multiple tags, and the same tag label can be applied to
//! multiple sessions.

use anyhow::{bail, Result};
use chrono::Utc;
use colored::Colorize;
use uuid::Uuid;

use crate::storage::{Database, Tag};

/// Arguments for the tag command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore tag abc123 bug-fix         Tag session with 'bug-fix'\n    \
    lore tag abc123 feature         Tag session with 'feature'\n    \
    lore tag abc123 wip --remove    Remove 'wip' tag from session")]
pub struct Args {
    /// Session ID prefix to tag
    #[arg(value_name = "SESSION")]
    pub session: String,

    /// Tag label to add or remove
    #[arg(value_name = "LABEL")]
    pub label: String,

    /// Remove the tag instead of adding it
    #[arg(long, short)]
    pub remove: bool,
}

/// Executes the tag command.
///
/// Adds or removes a tag from a session.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Find the session
    let session_id = find_session_by_prefix(&db, &args.session)?;
    let short_id = &session_id.to_string()[..8];

    if args.remove {
        // Remove the tag
        let deleted = db.delete_tag(&session_id, &args.label)?;
        if deleted {
            println!(
                "{}",
                format!(
                    "Removed tag '{}' from session {}",
                    args.label.yellow(),
                    short_id.cyan()
                )
                .green()
            );
        } else {
            println!(
                "{}",
                format!("Tag '{}' not found on session {}", args.label, short_id).dimmed()
            );
        }
    } else {
        // Add the tag
        if db.tag_exists(&session_id, &args.label)? {
            println!(
                "{}",
                format!(
                    "Session {} already has tag '{}'",
                    short_id.cyan(),
                    args.label.yellow()
                )
                .dimmed()
            );
        } else {
            let tag = Tag {
                id: Uuid::new_v4(),
                session_id,
                label: args.label.clone(),
                created_at: Utc::now(),
            };
            db.insert_tag(&tag)?;
            println!(
                "{}",
                format!(
                    "Added tag '{}' to session {}",
                    args.label.yellow(),
                    short_id.cyan()
                )
                .green()
            );
        }
    }

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
            let ids: Vec<String> = matching
                .iter()
                .map(|s| s.id.to_string()[..8].to_string())
                .collect();
            bail!(
                "Ambiguous session prefix '{}'. Matches: {}",
                id_prefix,
                ids.join(", ")
            )
        }
    }
}

//! Summarize command - add or view session summaries.
//!
//! Manages session summaries that provide concise descriptions of what
//! happened in a session. Summaries help with quickly understanding
//! session context when continuing work or reviewing history.

use anyhow::{bail, Result};
use chrono::Utc;
use colored::Colorize;
use uuid::Uuid;

use crate::storage::{Database, Summary};
use crate::summarize::{generate_summary, SummarizeError};

/// Arguments for the summarize command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore summarize abc123 \"Implemented auth feature\"  Add summary to session\n    \
    lore summarize abc123 --show                       View existing summary\n    \
    lore summarize abc123 --generate                   Generate summary via LLM")]
pub struct Args {
    /// Session ID prefix
    #[arg(value_name = "SESSION")]
    #[arg(
        long_help = "The session ID prefix to summarize. Must uniquely identify a\n\
        single session. Use 'lore sessions' to find session IDs."
    )]
    pub session: String,

    /// The summary text (required unless --show is used)
    #[arg(value_name = "SUMMARY")]
    #[arg(
        long_help = "The summary content describing what happened in the session.\n\
        Omit this to use --show and view the existing summary."
    )]
    pub summary: Option<String>,

    /// Show the existing summary without modifying
    #[arg(long)]
    #[arg(
        long_help = "Display the existing summary for the session instead of\n\
        adding or updating one."
    )]
    pub show: bool,

    /// Generate summary using a configured LLM provider
    #[arg(long)]
    #[arg(long_help = "Generate a summary using the configured LLM provider.\n\
        Requires summary_provider and a provider API key to be set via\n\
        'lore config set'. Cannot be used with manual summary text.")]
    pub generate: bool,
}

/// Executes the summarize command.
///
/// Adds, updates, or displays a summary for a session.
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
    let session_id = session.id;
    let session_short = &session.id.to_string()[..8];

    if args.show {
        // Show existing summary
        show_summary(&db, &session_id, session_short)?;
    } else if args.generate && args.summary.is_some() {
        bail!("Cannot use --generate with manual summary text.");
    } else if args.generate {
        // Generate summary via LLM
        let messages = db.get_messages(&session_id)?;
        match generate_summary(&messages) {
            Ok(summary_text) => {
                add_or_update_summary(&db, &session_id, session_short, &summary_text)?;
                println!(
                    "{} {}",
                    "Generated summary for session".green(),
                    session_short.cyan()
                );
                println!("{summary_text}");
            }
            Err(SummarizeError::NotConfigured) => {
                eprintln!(
                    "{}\n\n\
                     Configure a summary provider first:\n  \
                     lore config set summary_provider <anthropic|openai|openrouter>\n  \
                     lore config set summary_api_key_anthropic <key>\n  \
                     lore config set summary_api_key_openai <key>\n  \
                     lore config set summary_api_key_openrouter <key>",
                    "Summary provider not configured.".red()
                );
            }
            Err(SummarizeError::EmptySession) => {
                eprintln!("Session has no messages to summarize.");
            }
            Err(e) => {
                eprintln!("{} {e}", "Failed to generate summary:".red());
            }
        }
    } else if let Some(summary_text) = args.summary {
        // Add or update summary
        add_or_update_summary(&db, &session_id, session_short, &summary_text)?;
    } else {
        // No summary text, not showing, not generating - error
        bail!("Please provide a summary text, use --show to view, or --generate to auto-generate.");
    }

    Ok(())
}

/// Displays the existing summary for a session.
fn show_summary(db: &Database, session_id: &Uuid, session_short: &str) -> Result<()> {
    match db.get_summary(session_id)? {
        Some(summary) => {
            println!("{} {}", "Session".bold(), session_short.cyan());
            println!();
            println!("{}", "Summary:".bold());
            println!("{}", summary.content);
            println!();
            println!(
                "{}",
                format!(
                    "Last updated: {}",
                    summary.generated_at.format("%Y-%m-%d %H:%M:%S")
                )
                .dimmed()
            );
        }
        None => {
            println!(
                "{}",
                format!("No summary exists for session {session_short}").dimmed()
            );
            println!();
            println!(
                "{}",
                "Use 'lore summarize <session> \"<text>\"' to add one.".dimmed()
            );
        }
    }
    Ok(())
}

/// Adds a new summary or updates an existing one.
fn add_or_update_summary(
    db: &Database,
    session_id: &Uuid,
    session_short: &str,
    content: &str,
) -> Result<()> {
    // Check if a summary already exists
    let existing = db.get_summary(session_id)?;

    if existing.is_some() {
        // Update existing summary
        db.update_summary(session_id, content)?;
        println!(
            "{} session {}",
            "Updated summary for".green(),
            session_short.cyan()
        );
    } else {
        // Insert new summary
        let summary = Summary {
            id: Uuid::new_v4(),
            session_id: *session_id,
            content: content.to_string(),
            generated_at: Utc::now(),
        };
        db.insert_summary(&summary)?;
        println!(
            "{} session {}",
            "Summary saved for".green(),
            session_short.cyan()
        );
    }

    Ok(())
}

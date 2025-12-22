//! Show command - display session details.
//!
//! Displays the full conversation history for a session, or lists
//! sessions linked to a specific commit. Supports truncation of
//! long messages and optional display of AI thinking blocks.

use std::env;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::git;
use crate::storage::{ContentBlock, Database, MessageContent, MessageRole};

/// Safely truncates a string to at most `max_bytes` bytes at a character boundary.
///
/// Ensures the truncation does not split a multi-byte UTF-8 character.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last valid character boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Arguments for the show command.
#[derive(clap::Args)]
pub struct Args {
    /// Session ID prefix or commit SHA to look up.
    pub target: String,

    /// Interpret target as a commit SHA and show linked sessions.
    #[arg(long)]
    pub commit: bool,

    /// Show full message content without truncation.
    #[arg(long)]
    pub full: bool,

    /// Include AI thinking blocks in the output.
    #[arg(long)]
    pub thinking: bool,
}

/// Executes the show command.
///
/// Either displays a session's conversation or lists sessions
/// linked to a commit, depending on the --commit flag.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    if args.commit {
        // Show sessions linked to a commit
        show_commit_sessions(&db, &args.target)?;
    } else {
        // Show a specific session
        show_session(&db, &args.target, args.full, args.thinking)?;
    }

    Ok(())
}

fn show_session(db: &Database, id_prefix: &str, full: bool, show_thinking: bool) -> Result<()> {
    // Find session by ID prefix
    let sessions = db.list_sessions(100, None)?;
    let session = sessions
        .iter()
        .find(|s| s.id.to_string().starts_with(id_prefix))
        .context(format!("No session found matching '{id_prefix}'"))?;

    // Header
    println!("{} {}", "Session".bold(), session.id.to_string().cyan());
    println!();
    println!("  {}  {}", "Tool:".dimmed(), session.tool);
    if let Some(ref v) = session.tool_version {
        println!("  {}  {}", "Version:".dimmed(), v);
    }
    if let Some(ref m) = session.model {
        println!("  {}  {}", "Model:".dimmed(), m);
    }
    println!(
        "  {}  {}",
        "Started:".dimmed(),
        session.started_at.format("%Y-%m-%d %H:%M:%S")
    );
    if let Some(ended) = session.ended_at {
        let duration = ended.signed_duration_since(session.started_at);
        println!(
            "  {}  {} ({} minutes)",
            "Ended:".dimmed(),
            ended.format("%Y-%m-%d %H:%M:%S"),
            duration.num_minutes()
        );
    }
    println!("  {}  {}", "Messages:".dimmed(), session.message_count);
    println!("  {}  {}", "Directory:".dimmed(), session.working_directory);
    if let Some(ref branch) = session.git_branch {
        println!("  {}  {}", "Branch:".dimmed(), branch);
    }

    // Check for links
    let links = db.get_links_by_session(&session.id)?;
    if !links.is_empty() {
        println!();
        println!("{}", "Linked to:".bold());
        for link in links {
            if let Some(ref sha) = link.commit_sha {
                let short_sha = &sha[..8.min(sha.len())];
                println!("  {} {}", "commit".dimmed(), short_sha.yellow());
            }
        }
    }

    // Messages
    println!();
    println!("{}", "Conversation:".bold());
    println!();

    let messages = db.get_messages(&session.id)?;
    for msg in messages {
        let role_str = match msg.role {
            MessageRole::User => "Human".green().bold(),
            MessageRole::Assistant => "Assistant".blue().bold(),
            MessageRole::System => "System".yellow().bold(),
        };

        let time = msg.timestamp.format("%H:%M:%S").to_string();
        println!("[{} {}]", role_str, time.dimmed());

        // Format content
        match &msg.content {
            MessageContent::Text(text) => {
                let display = if full || text.len() < 500 {
                    text.clone()
                } else {
                    format!("{}...", truncate_str(text, 500))
                };
                println!("{display}");
            }
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            let display = if full || text.len() < 500 {
                                text.clone()
                            } else {
                                format!("{}...", truncate_str(text, 500))
                            };
                            println!("{display}");
                        }
                        ContentBlock::Thinking { thinking } => {
                            if show_thinking {
                                println!(
                                    "{} {}",
                                    "💭".dimmed(),
                                    thinking.dimmed()
                                );
                            }
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            println!(
                                "{} {}",
                                format!("[Tool: {name}]").magenta(),
                                serde_json::to_string(input)
                                    .unwrap_or_default()
                                    .dimmed()
                            );
                        }
                        ContentBlock::ToolResult {
                            content, is_error, ..
                        } => {
                            let label = if *is_error { "Error" } else { "Result" };
                            let color_content = if *is_error {
                                content.red().to_string()
                            } else {
                                content.dimmed().to_string()
                            };
                            let display = if full || content.len() < 200 {
                                color_content
                            } else {
                                format!("{}...", truncate_str(&color_content, 200))
                            };
                            println!("{} {}", format!("[{label}]").dimmed(), display);
                        }
                    }
                }
            }
        }
        println!();
    }

    Ok(())
}

/// Resolves a commit reference to a SHA and returns both the resolved SHA
/// and any display information (like showing it resolved from HEAD).
///
/// Tries to resolve git refs first, falls back to treating the input as a
/// partial SHA if not in a git repo or if resolution fails.
fn resolve_commit_reference(commit: &str) -> (String, Option<String>) {
    // Try to resolve as a git reference from the current directory
    let cwd = env::current_dir().ok();

    if let Some(ref path) = cwd {
        if let Ok(resolved_sha) = git::resolve_commit_ref(path, commit) {
            // Successfully resolved - check if we resolved from a symbolic ref
            if commit != resolved_sha && !resolved_sha.starts_with(commit) {
                // Input was a symbolic ref (HEAD, branch name, etc.)
                return (resolved_sha, Some(commit.to_string()));
            }
            // Input was a SHA (partial or full) that got expanded
            return (resolved_sha, None);
        }
    }

    // Could not resolve as git ref - treat as partial SHA for database lookup
    (commit.to_string(), None)
}

fn show_commit_sessions(db: &Database, commit: &str) -> Result<()> {
    // Resolve the commit reference (handles HEAD, branch names, etc.)
    let (resolved_sha, ref_name) = resolve_commit_reference(commit);

    // Query database with the resolved SHA
    let links = db.get_links_by_commit(&resolved_sha)?;

    if links.is_empty() {
        let display = if let Some(ref name) = ref_name {
            format!("'{name}' ({})", &resolved_sha[..8.min(resolved_sha.len())])
        } else {
            format!("'{commit}'")
        };
        println!(
            "{}",
            format!("No sessions linked to commit {display}").dimmed()
        );
        return Ok(());
    }

    // Check for multiple SHA matches and warn if ambiguous
    let unique_shas: std::collections::HashSet<_> = links
        .iter()
        .filter_map(|l| l.commit_sha.as_ref())
        .collect();

    if unique_shas.len() > 1 {
        eprintln!(
            "{}",
            format!(
                "Warning: Partial SHA '{}' matches {} commits",
                &resolved_sha[..8.min(resolved_sha.len())],
                unique_shas.len()
            )
            .yellow()
        );
    }

    // Get commit info for the header (if in a git repo)
    let cwd = env::current_dir().ok();
    let commit_info = cwd
        .as_ref()
        .and_then(|p| git::get_commit_info(p, &resolved_sha).ok());

    // Print commit header
    let short_sha = &resolved_sha[..8.min(resolved_sha.len())];
    if let Some(ref info) = commit_info {
        let ref_display = ref_name
            .as_ref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();

        println!(
            "{} {}{}",
            "Commit".bold(),
            short_sha.yellow(),
            ref_display.dimmed()
        );
        println!("  \"{}\"", info.summary);
        println!("  {}", info.timestamp.format("%Y-%m-%d %H:%M"));
    } else {
        // Fallback if not in a git repo or commit not found
        let ref_display = ref_name
            .as_ref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        println!(
            "{} {}{}",
            "Commit".bold(),
            short_sha.yellow(),
            ref_display.dimmed()
        );
    }

    println!();
    println!(
        "{}",
        format!("Linked sessions ({}):", links.len()).bold()
    );

    for link in &links {
        if let Some(session) = db.get_session(&link.session_id)? {
            let id_short = &session.id.to_string()[..8];
            let started = session.started_at.format("%Y-%m-%d %H:%M").to_string();

            println!(
                "  {}  {}  {} messages",
                id_short.cyan(),
                started.dimmed(),
                session.message_count
            );

            if let Some(conf) = link.confidence {
                println!(
                    "    {} {:.0}%",
                    "confidence:".dimmed(),
                    conf * 100.0
                );
            }
        }
    }

    println!();
    println!(
        "{}",
        "Use 'lore show <session-id>' to view session details".dimmed()
    );

    Ok(())
}

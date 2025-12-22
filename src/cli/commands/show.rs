//! Show command - display session details

use anyhow::{Context, Result};
use colored::Colorize;

use crate::storage::{ContentBlock, Database, MessageContent, MessageRole};

/// Safely truncate a string to at most `max_bytes` bytes at a character boundary
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

#[derive(clap::Args)]
pub struct Args {
    /// Session ID (first 8 chars) or commit SHA
    pub target: String,

    /// Show linked sessions for a commit
    #[arg(long)]
    pub commit: bool,

    /// Show full content (don't truncate)
    #[arg(long)]
    pub full: bool,

    /// Include thinking blocks
    #[arg(long)]
    pub thinking: bool,
}

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
        .context(format!("No session found matching '{}'", id_prefix))?;

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
                println!("{}", display);
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
                            println!("{}", display);
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
                                format!("[Tool: {}]", name).magenta(),
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
                            println!("{} {}", format!("[{}]", label).dimmed(), display);
                        }
                    }
                }
            }
        }
        println!();
    }

    Ok(())
}

fn show_commit_sessions(db: &Database, commit: &str) -> Result<()> {
    let links = db.get_links_by_commit(commit)?;

    if links.is_empty() {
        println!(
            "{}",
            format!("No sessions linked to commit '{}'", commit).dimmed()
        );
        return Ok(());
    }

    println!(
        "{}",
        format!("Sessions linked to commit {}:", &commit[..8.min(commit.len())]).bold()
    );
    println!();

    for link in links {
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

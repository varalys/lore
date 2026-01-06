//! Show command - display session details.
//!
//! Displays the full conversation history for a session, or lists
//! sessions linked to a specific commit. Supports truncation of
//! long messages and optional display of AI thinking blocks.
//!
//! Supports multiple output formats:
//! - Text: colored terminal output (default)
//! - JSON: machine-readable structured output
//! - Markdown: formatted for documentation or issue tracking

use std::env;

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::git;
use crate::storage::{ContentBlock, Database, Message, MessageContent, MessageRole, Session, Tag};

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
#[command(after_help = "EXAMPLES:\n    \
    lore show abc123                View session by ID prefix\n    \
    lore show abc123 --full         Show full message content\n    \
    lore show abc123 --thinking     Include AI thinking blocks\n    \
    lore show --commit HEAD         List sessions linked to HEAD\n    \
    lore show --commit abc123       List sessions linked to commit\n    \
    lore show abc123 -f markdown    Output as markdown")]
pub struct Args {
    /// Session ID prefix or commit SHA to look up
    #[arg(value_name = "ID")]
    #[arg(long_help = "The target to look up. By default this is treated as a\n\
        session ID prefix. Use --commit to interpret it as a git\n\
        commit reference (SHA, HEAD, branch name, etc).")]
    pub target: String,

    /// Treat the target as a commit and show linked sessions
    #[arg(long)]
    #[arg(
        long_help = "Interpret the target as a git commit reference instead of\n\
        a session ID. Shows all sessions linked to that commit.\n\
        Accepts SHAs, HEAD, branch names, or any git ref."
    )]
    pub commit: bool,

    /// Show full message content without truncation
    #[arg(long)]
    #[arg(
        long_help = "By default, long messages are truncated for readability.\n\
        Use this flag to show the complete content of all messages."
    )]
    pub full: bool,

    /// Include AI thinking blocks in output
    #[arg(long)]
    #[arg(
        long_help = "Include the AI's internal thinking/reasoning blocks in the\n\
        output. These are normally hidden but can provide insight\n\
        into the AI's decision-making process."
    )]
    pub thinking: bool,

    /// Output format: text (default), json, or markdown
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for a session with its messages.
#[derive(Serialize)]
struct SessionOutput {
    session: Session,
    messages: Vec<Message>,
    links: Vec<LinkInfo>,
    tags: Vec<String>,
    summary: Option<String>,
}

/// Simplified link info for JSON output.
#[derive(Serialize)]
struct LinkInfo {
    commit_sha: Option<String>,
    confidence: Option<f64>,
}

/// Executes the show command.
///
/// Either displays a session's conversation or lists sessions
/// linked to a commit, depending on the --commit flag.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    if args.commit {
        // Show sessions linked to a commit
        show_commit_sessions(&db, &args.target, args.format)?;
    } else {
        // Show a specific session
        show_session(&db, &args.target, args.full, args.thinking, args.format)?;
    }

    Ok(())
}

fn show_session(
    db: &Database,
    id_prefix: &str,
    full: bool,
    show_thinking: bool,
    format: OutputFormat,
) -> Result<()> {
    // Find session by ID prefix using efficient database lookup
    let session = match db.find_session_by_id_prefix(id_prefix)? {
        Some(s) => s,
        None => {
            // Check if database is empty for a better error message
            if db.session_count()? == 0 {
                anyhow::bail!(
                    "No session found matching '{id_prefix}'. No sessions in database. \
                     Run 'lore import' to import sessions from Claude Code."
                );
            } else {
                anyhow::bail!(
                    "No session found matching '{id_prefix}'. \
                     Run 'lore sessions' to list available sessions."
                );
            }
        }
    };

    let messages = db.get_messages(&session.id)?;
    let links = db.get_links_by_session(&session.id)?;
    let tags = db.get_tags(&session.id)?;
    let summary = db.get_summary(&session.id)?;

    match format {
        OutputFormat::Json => {
            let output = SessionOutput {
                session: session.clone(),
                messages,
                links: links
                    .iter()
                    .map(|l| LinkInfo {
                        commit_sha: l.commit_sha.clone(),
                        confidence: l.confidence,
                    })
                    .collect(),
                tags: tags.iter().map(|t| t.label.clone()).collect(),
                summary: summary.map(|s| s.content),
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Markdown => {
            print_session_markdown(
                &session,
                &messages,
                &links,
                &tags,
                &summary,
                full,
                show_thinking,
            );
        }
        OutputFormat::Text => {
            print_session_text(
                &session,
                &messages,
                &links,
                &tags,
                &summary,
                full,
                show_thinking,
            );
        }
    }

    Ok(())
}

/// Prints session details in text format with colors.
fn print_session_text(
    session: &Session,
    messages: &[Message],
    links: &[crate::storage::SessionLink],
    tags: &[Tag],
    summary: &Option<crate::storage::Summary>,
    full: bool,
    show_thinking: bool,
) {
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

    // Display tags
    if !tags.is_empty() {
        let tag_labels: Vec<String> = tags.iter().map(|t| t.label.yellow().to_string()).collect();
        println!("  {}  {}", "Tags:".dimmed(), tag_labels.join(", "));
    }

    // Display summary
    if let Some(ref s) = summary {
        println!();
        println!("{}", "Summary:".bold());
        println!("  {}", s.content);
    }

    // Check for links
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

    for msg in messages {
        let role_str = match msg.role {
            MessageRole::User => "Human".green().bold(),
            MessageRole::Assistant => "Assistant".blue().bold(),
            MessageRole::System => "System".yellow().bold(),
        };

        let time = msg.timestamp.format("%H:%M:%S").to_string();
        println!("[{} {}]", role_str, time.dimmed());

        print_message_content_text(&msg.content, full, show_thinking);
        println!();
    }
}

/// Prints message content in text format.
fn print_message_content_text(content: &MessageContent, full: bool, show_thinking: bool) {
    match content {
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
                            println!("{} {}", "<thinking>".dimmed(), thinking.dimmed());
                        }
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        println!(
                            "{} {}",
                            format!("[Tool: {name}]").magenta(),
                            serde_json::to_string(input).unwrap_or_default().dimmed()
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
}

/// Prints session details in markdown format.
fn print_session_markdown(
    session: &Session,
    messages: &[Message],
    links: &[crate::storage::SessionLink],
    tags: &[Tag],
    summary: &Option<crate::storage::Summary>,
    full: bool,
    show_thinking: bool,
) {
    // Header
    println!("# Session {}", session.id);
    println!();

    // Metadata table
    println!("| Property | Value |");
    println!("|----------|-------|");
    println!("| Tool | {} |", session.tool);
    if let Some(ref v) = session.tool_version {
        println!("| Version | {v} |");
    }
    if let Some(ref m) = session.model {
        println!("| Model | {m} |");
    }
    println!(
        "| Started | {} |",
        session.started_at.format("%Y-%m-%d %H:%M:%S")
    );
    if let Some(ended) = session.ended_at {
        let duration = ended.signed_duration_since(session.started_at);
        println!(
            "| Ended | {} ({} minutes) |",
            ended.format("%Y-%m-%d %H:%M:%S"),
            duration.num_minutes()
        );
    }
    println!("| Messages | {} |", session.message_count);
    println!("| Directory | `{}` |", session.working_directory);
    if let Some(ref branch) = session.git_branch {
        println!("| Branch | `{branch}` |");
    }
    if !tags.is_empty() {
        let tag_labels: Vec<&str> = tags.iter().map(|t| t.label.as_str()).collect();
        println!("| Tags | {} |", tag_labels.join(", "));
    }
    println!();

    // Summary
    if let Some(ref s) = summary {
        println!("## Summary");
        println!();
        println!("{}", s.content);
        println!();
    }

    // Links
    if !links.is_empty() {
        println!("## Linked Commits");
        println!();
        for link in links {
            if let Some(ref sha) = link.commit_sha {
                let short_sha = &sha[..8.min(sha.len())];
                print!("- `{short_sha}`");
                if let Some(conf) = link.confidence {
                    print!(" (confidence: {:.0}%)", conf * 100.0);
                }
                println!();
            }
        }
        println!();
    }

    // Conversation
    println!("## Conversation");
    println!();

    for msg in messages {
        let role = match msg.role {
            MessageRole::User => "Human",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
        };

        let time = msg.timestamp.format("%H:%M:%S").to_string();
        println!("### [{role}] {time}");
        println!();

        print_message_content_markdown(&msg.content, full, show_thinking);
        println!();
    }
}

/// Prints message content in markdown format.
fn print_message_content_markdown(content: &MessageContent, full: bool, show_thinking: bool) {
    match content {
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
                            println!("<details>");
                            println!("<summary>Thinking</summary>");
                            println!();
                            println!("{thinking}");
                            println!();
                            println!("</details>");
                            println!();
                        }
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        println!("**Tool: {name}**");
                        println!();
                        println!("```json");
                        println!(
                            "{}",
                            serde_json::to_string_pretty(input).unwrap_or_default()
                        );
                        println!("```");
                        println!();
                    }
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        let label = if *is_error { "Error" } else { "Result" };
                        let display = if full || content.len() < 200 {
                            content.clone()
                        } else {
                            format!("{}...", truncate_str(content, 200))
                        };
                        println!("**{label}:**");
                        println!();
                        println!("```");
                        println!("{display}");
                        println!("```");
                        println!();
                    }
                }
            }
        }
    }
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

/// Output structure for commit sessions in JSON format.
#[derive(Serialize)]
struct CommitSessionsOutput {
    commit_sha: String,
    ref_name: Option<String>,
    commit_summary: Option<String>,
    commit_timestamp: Option<String>,
    sessions: Vec<CommitSessionInfo>,
}

/// Session info for commit sessions JSON output.
#[derive(Serialize)]
struct CommitSessionInfo {
    session_id: String,
    started_at: String,
    message_count: i32,
    confidence: Option<f64>,
}

fn show_commit_sessions(db: &Database, commit: &str, format: OutputFormat) -> Result<()> {
    // Resolve the commit reference (handles HEAD, branch names, etc.)
    let (resolved_sha, ref_name) = resolve_commit_reference(commit);

    // Query database with the resolved SHA
    let links = db.get_links_by_commit(&resolved_sha)?;

    if links.is_empty() {
        match format {
            OutputFormat::Json => {
                let output = CommitSessionsOutput {
                    commit_sha: resolved_sha.clone(),
                    ref_name,
                    commit_summary: None,
                    commit_timestamp: None,
                    sessions: vec![],
                };
                let json = serde_json::to_string_pretty(&output)?;
                println!("{json}");
            }
            OutputFormat::Text | OutputFormat::Markdown => {
                let display = if let Some(ref name) = ref_name {
                    format!("'{name}' ({})", &resolved_sha[..8.min(resolved_sha.len())])
                } else {
                    format!("'{commit}'")
                };
                println!(
                    "{}",
                    format!("No sessions linked to commit {display}").dimmed()
                );
            }
        }
        return Ok(());
    }

    // Check for multiple SHA matches and warn if ambiguous (only for text output)
    if matches!(format, OutputFormat::Text) {
        let unique_shas: std::collections::HashSet<_> =
            links.iter().filter_map(|l| l.commit_sha.as_ref()).collect();

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
    }

    // Get commit info for the header (if in a git repo)
    let cwd = env::current_dir().ok();
    let commit_info = cwd
        .as_ref()
        .and_then(|p| git::get_commit_info(p, &resolved_sha).ok());

    // Collect session info
    let mut session_infos = Vec::new();
    for link in &links {
        if let Some(session) = db.get_session(&link.session_id)? {
            session_infos.push((session, link.confidence));
        }
    }

    match format {
        OutputFormat::Json => {
            let output = CommitSessionsOutput {
                commit_sha: resolved_sha.clone(),
                ref_name,
                commit_summary: commit_info.as_ref().map(|i| i.summary.clone()),
                commit_timestamp: commit_info.as_ref().map(|i| i.timestamp.to_rfc3339()),
                sessions: session_infos
                    .iter()
                    .map(|(s, conf)| CommitSessionInfo {
                        session_id: s.id.to_string(),
                        started_at: s.started_at.to_rfc3339(),
                        message_count: s.message_count,
                        confidence: *conf,
                    })
                    .collect(),
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Markdown => {
            let short_sha = &resolved_sha[..8.min(resolved_sha.len())];
            println!("# Commit `{short_sha}`");
            println!();

            if let Some(ref info) = commit_info {
                println!("**Summary:** {}", info.summary);
                println!();
                println!("**Date:** {}", info.timestamp.format("%Y-%m-%d %H:%M"));
                println!();
            }

            println!("## Linked Sessions ({})", session_infos.len());
            println!();
            println!("| Session ID | Started | Messages | Confidence |");
            println!("|------------|---------|----------|------------|");

            for (session, conf) in &session_infos {
                let id_short = &session.id.to_string()[..8];
                let started = session.started_at.format("%Y-%m-%d %H:%M").to_string();
                let conf_str = conf
                    .map(|c| format!("{:.0}%", c * 100.0))
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "| `{id_short}` | {started} | {} | {conf_str} |",
                    session.message_count
                );
            }
        }
        OutputFormat::Text => {
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
                format!("Linked sessions ({}):", session_infos.len()).bold()
            );

            for (session, conf) in &session_infos {
                let id_short = &session.id.to_string()[..8];
                let started = session.started_at.format("%Y-%m-%d %H:%M").to_string();

                println!(
                    "  {}  {}  {} messages",
                    id_short.cyan(),
                    started.dimmed(),
                    session.message_count
                );

                if let Some(c) = conf {
                    println!("    {} {:.0}%", "confidence:".dimmed(), c * 100.0);
                }
            }

            println!();
            println!(
                "{}",
                "Use 'lore show <session-id>' to view session details".dimmed()
            );
        }
    }

    Ok(())
}

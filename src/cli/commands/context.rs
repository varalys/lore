//! Context command - show recent sessions for quick orientation.
//!
//! Provides a summary of recent sessions for the current repository,
//! helping users quickly understand the recent development context.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::storage::{Annotation, Database, MessageContent, Summary, Tag};

/// Maximum length for message preview snippets.
const MESSAGE_PREVIEW_LENGTH: usize = 200;

/// Number of recent messages to show in --last view.
const LAST_MESSAGES_COUNT: usize = 3;

/// Arguments for the context command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore context              Show last 5 sessions for current repo\n    \
    lore context --repo /path Show sessions for specific path\n    \
    lore context --last       Show detailed summary of most recent session\n    \
    lore context --json       Output as JSON")]
pub struct Args {
    /// Filter to sessions in this directory (prefix match)
    #[arg(short, long, value_name = "PATH")]
    pub repo: Option<String>,

    /// Show detailed summary of only the most recent session
    #[arg(long)]
    pub last: bool,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for a session summary.
#[derive(Serialize)]
struct SessionSummary {
    id: String,
    id_short: String,
    tool: String,
    started_at: String,
    message_count: i32,
    linked_commits: Vec<String>,
    working_directory: String,
    git_branch: Option<String>,
}

/// JSON output structure for detailed last session view.
#[derive(Serialize)]
struct DetailedSessionOutput {
    id: String,
    id_short: String,
    tool: String,
    started_at: String,
    message_count: i32,
    linked_commits: Vec<String>,
    working_directory: String,
    git_branch: Option<String>,
    summary: Option<String>,
    annotations: Vec<AnnotationInfo>,
    tags: Vec<String>,
    recent_messages: Vec<MessagePreview>,
}

/// Annotation info for JSON output.
#[derive(Serialize)]
struct AnnotationInfo {
    content: String,
    created_at: String,
}

/// Message preview for JSON output.
#[derive(Serialize)]
struct MessagePreview {
    role: String,
    content: String,
    timestamp: String,
}

/// JSON output structure for the context command.
#[derive(Serialize)]
struct ContextOutput {
    working_directory: String,
    sessions: Vec<SessionSummary>,
}

/// Executes the context command.
///
/// Shows a summary of recent sessions for the current or specified repository.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Resolve repo path
    let working_dir = match args.repo {
        Some(ref r) if r == "." => std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| r.clone()),
        Some(r) => r,
        None => std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
    };

    let limit = if args.last { 1 } else { 5 };
    let sessions = db.list_sessions(limit, Some(&working_dir))?;

    if sessions.is_empty() {
        match args.format {
            OutputFormat::Json => {
                let output = ContextOutput {
                    working_directory: working_dir,
                    sessions: vec![],
                };
                let json = serde_json::to_string_pretty(&output)?;
                println!("{json}");
            }
            OutputFormat::Text | OutputFormat::Markdown => {
                println!("{}", "No sessions found for this directory.".dimmed());
                println!();
                println!("Run 'lore import' to import sessions from AI coding tools.");
            }
        }
        return Ok(());
    }

    // Build session summaries with linked commits
    let mut summaries = Vec::new();
    for session in &sessions {
        let links = db.get_links_by_session(&session.id)?;
        let linked_commits: Vec<String> = links
            .iter()
            .filter_map(|l| l.commit_sha.as_ref())
            .map(|sha| sha[..8.min(sha.len())].to_string())
            .collect();

        summaries.push(SessionSummary {
            id: session.id.to_string(),
            id_short: session.id.to_string()[..8].to_string(),
            tool: session.tool.clone(),
            started_at: session.started_at.to_rfc3339(),
            message_count: session.message_count,
            linked_commits,
            working_directory: session.working_directory.clone(),
            git_branch: session.git_branch.clone(),
        });
    }

    match args.format {
        OutputFormat::Json => {
            if args.last {
                // Detailed JSON output for --last
                let session = &sessions[0];
                let summary_struct = &summaries[0];
                output_detailed_json(&db, session, summary_struct)?;
            } else {
                let output = ContextOutput {
                    working_directory: working_dir,
                    sessions: summaries,
                };
                let json = serde_json::to_string_pretty(&output)?;
                println!("{json}");
            }
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            if args.last {
                // Detailed view of the most recent session
                let summary = &summaries[0];
                let session = &sessions[0];

                println!("{} {}", "Session".bold(), summary.id_short.cyan());
                println!();
                println!("  {}  {}", "Tool:".dimmed(), session.tool);
                println!(
                    "  {}  {}",
                    "Started:".dimmed(),
                    session.started_at.format("%Y-%m-%d %H:%M:%S")
                );
                println!("  {}  {}", "Messages:".dimmed(), session.message_count);
                println!("  {}  {}", "Directory:".dimmed(), session.working_directory);
                if let Some(ref branch) = session.git_branch {
                    println!("  {}  {}", "Branch:".dimmed(), branch);
                }

                // Show linked commits
                if !summary.linked_commits.is_empty() {
                    println!();
                    println!("{}", "Linked commits:".bold());
                    for sha in &summary.linked_commits {
                        println!("  {} {}", "commit".dimmed(), sha.yellow());
                    }
                }

                // Show summary if available
                if let Some(session_summary) = db.get_summary(&session.id)? {
                    println!();
                    println!("{}", "Summary:".bold());
                    println!("  {}", session_summary.content);
                }

                // Show tags if any
                let tags = db.get_tags(&session.id)?;
                if !tags.is_empty() {
                    println!();
                    println!("{}", "Tags:".bold());
                    print_tags(&tags);
                }

                // Show recent annotations if any
                let annotations = db.get_annotations(&session.id)?;
                if !annotations.is_empty() {
                    println!();
                    println!("{}", "Recent annotations:".bold());
                    print_annotations(&annotations);
                }

                // Show last few messages as a preview
                let messages = db.get_messages(&session.id)?;
                if !messages.is_empty() {
                    println!();
                    println!("{}", "Recent messages:".bold());
                    let start = messages.len().saturating_sub(LAST_MESSAGES_COUNT);
                    for msg in &messages[start..] {
                        let role = match msg.role {
                            crate::storage::MessageRole::User => "Human".green(),
                            crate::storage::MessageRole::Assistant => "Assistant".blue(),
                            crate::storage::MessageRole::System => "System".yellow(),
                        };
                        let content = truncate_content(&msg.content, MESSAGE_PREVIEW_LENGTH);
                        println!("  [{}] {}", role, content.dimmed());
                    }
                }

                // Hint for continuing
                println!();
                println!(
                    "{}",
                    format!(
                        "Use 'lore show {}' for full conversation history.",
                        &summary.id_short
                    )
                    .dimmed()
                );
            } else {
                // Summary table view
                println!(
                    "{}",
                    format!(
                        "{:<8}  {:<12}  {:>8}  {:>8}  {}",
                        "ID", "TOOL", "MESSAGES", "COMMITS", "STARTED"
                    )
                    .bold()
                );

                for summary in &summaries {
                    let commits_display = if summary.linked_commits.is_empty() {
                        "-".to_string()
                    } else {
                        summary.linked_commits.len().to_string()
                    };

                    // Parse the RFC3339 timestamp for display
                    let started_display = chrono::DateTime::parse_from_rfc3339(&summary.started_at)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|_| summary.started_at.clone());

                    println!(
                        "{:<8}  {:<12}  {:>8}  {:>8}  {}",
                        summary.id_short.cyan(),
                        summary.tool,
                        summary.message_count,
                        commits_display,
                        started_display.dimmed()
                    );
                }
            }
        }
    }

    Ok(())
}

/// Outputs detailed JSON for a single session in --last mode.
fn output_detailed_json(
    db: &Database,
    session: &crate::storage::Session,
    summary_struct: &SessionSummary,
) -> Result<()> {
    let session_summary: Option<Summary> = db.get_summary(&session.id)?;
    let annotations = db.get_annotations(&session.id)?;
    let tags = db.get_tags(&session.id)?;
    let messages = db.get_messages(&session.id)?;

    // Get last few messages
    let start = messages.len().saturating_sub(LAST_MESSAGES_COUNT);
    let recent_messages: Vec<MessagePreview> = messages[start..]
        .iter()
        .map(|msg| MessagePreview {
            role: msg.role.to_string(),
            content: truncate_content(&msg.content, MESSAGE_PREVIEW_LENGTH),
            timestamp: msg.timestamp.to_rfc3339(),
        })
        .collect();

    let output = DetailedSessionOutput {
        id: summary_struct.id.clone(),
        id_short: summary_struct.id_short.clone(),
        tool: summary_struct.tool.clone(),
        started_at: summary_struct.started_at.clone(),
        message_count: summary_struct.message_count,
        linked_commits: summary_struct.linked_commits.clone(),
        working_directory: summary_struct.working_directory.clone(),
        git_branch: summary_struct.git_branch.clone(),
        summary: session_summary.map(|s| s.content),
        annotations: annotations
            .into_iter()
            .map(|a| AnnotationInfo {
                content: a.content,
                created_at: a.created_at.to_rfc3339(),
            })
            .collect(),
        tags: tags.into_iter().map(|t| t.label).collect(),
        recent_messages,
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{json}");
    Ok(())
}

/// Prints tags in a formatted list.
fn print_tags(tags: &[Tag]) {
    let labels: Vec<String> = tags.iter().map(|t| t.label.yellow().to_string()).collect();
    println!("  {}", labels.join(", "));
}

/// Prints annotations in a formatted list.
fn print_annotations(annotations: &[Annotation]) {
    // Show last 3 annotations
    let start = annotations.len().saturating_sub(3);
    for ann in &annotations[start..] {
        let time = ann.created_at.format("%Y-%m-%d %H:%M").to_string();
        println!("  [{}] {}", time.dimmed(), ann.content);
    }
}

/// Truncates message content for preview display.
fn truncate_content(content: &MessageContent, max_len: usize) -> String {
    let text = content.text();
    // Replace newlines with spaces for single-line preview
    let text = text.replace('\n', " ");
    // Collapse multiple spaces
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if text.len() <= max_len {
        text
    } else {
        format!("{}...", &text[..max_len.saturating_sub(3)])
    }
}

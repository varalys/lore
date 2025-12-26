//! Search command - search session content.
//!
//! Provides full-text search across session messages using SQLite FTS5.
//! Supports filtering by repository, date, tool, project, branch, and message role.
//! Also searches session metadata (working directory, branch, tool name).

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::storage::db::Database;
use crate::storage::models::{
    ContextMessage, MatchWithContext, MessageRole, SearchOptions, SearchResult,
    SearchResultWithContext,
};

/// Arguments for the search command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore search \"auth\"                        Search for 'auth' in messages\n    \
    lore search \"bug fix\" --limit 20          Show up to 20 results\n    \
    lore search api --since 7d                Last 7 days only\n    \
    lore search error --role assistant        Only AI responses\n    \
    lore search test --repo /path             Filter by repository\n    \
    lore search auth --tool claude-code       Filter by AI tool\n    \
    lore search api --project myapp           Filter by project name\n    \
    lore search fix --branch feat/auth        Filter by git branch\n    \
    lore search bug --context 2               Show 2 messages of context")]
pub struct Args {
    /// Text to search for in session messages and metadata
    #[arg(value_name = "QUERY")]
    #[arg(
        long_help = "The text to search for. Uses SQLite FTS5 full-text search,\n\
        which supports word matching and basic boolean operators.\n\
        Searches message content, session metadata (project, branch, tool).\n\
        The search index is built automatically on first use."
    )]
    pub query: String,

    /// Maximum number of results to return
    #[arg(short, long, default_value = "10", value_name = "N")]
    pub limit: usize,

    /// Filter by repository path prefix
    #[arg(long, value_name = "PATH")]
    #[arg(
        long_help = "Only search sessions from repositories matching this path\n\
        prefix. Useful for narrowing results to a specific project."
    )]
    pub repo: Option<String>,

    /// Filter by AI tool name
    #[arg(long, value_name = "TOOL")]
    #[arg(long_help = "Only search sessions from a specific AI tool:\n\
        - claude-code: Claude Code CLI sessions\n\
        - aider: Aider sessions\n\
        - gemini: Gemini sessions\n\
        - cursor: Cursor editor sessions")]
    pub tool: Option<String>,

    /// Filter by date - sessions after this date (e.g., 7d, 2w, 1m, or 2024-01-01)
    #[arg(long, value_name = "DATE")]
    #[arg(long_help = "Only search sessions from after this date. Accepts:\n\
        - Relative: 7d (days), 2w (weeks), 1m (months)\n\
        - Absolute: 2024-01-15 (ISO date format)")]
    pub since: Option<String>,

    /// Filter by date - sessions before this date (e.g., 7d, 2w, 1m, or 2024-12-31)
    #[arg(long, value_name = "DATE")]
    #[arg(long_help = "Only search sessions from before this date. Accepts:\n\
        - Relative: 7d (days), 2w (weeks), 1m (months)\n\
        - Absolute: 2024-12-31 (ISO date format)")]
    pub until: Option<String>,

    /// Filter by project/directory name (partial match)
    #[arg(long, value_name = "NAME")]
    #[arg(long_help = "Filter by project name (matches working directory).\n\
        Supports partial matching, e.g., --project myapp matches\n\
        /home/user/projects/myapp-backend")]
    pub project: Option<String>,

    /// Filter by git branch name (partial match)
    #[arg(long, value_name = "BRANCH")]
    #[arg(long_help = "Filter by git branch name.\n\
        Supports partial matching, e.g., --branch feat matches\n\
        feat/authentication, feat/api, etc.")]
    pub branch: Option<String>,

    /// Filter by message role (user, assistant, system)
    #[arg(long, value_name = "ROLE")]
    #[arg(long_help = "Only search messages from a specific role:\n\
        - user: human messages\n\
        - assistant: AI responses\n\
        - system: system prompts")]
    pub role: Option<String>,

    /// Number of context messages to show before and after matches
    #[arg(short = 'C', long, default_value = "1", value_name = "N")]
    #[arg(long_help = "Show N messages before and after each match for context.\n\
        Use 0 to disable context. Default is 1.")]
    pub context: usize,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for search results with context.
#[derive(Serialize)]
struct SearchOutputWithContext {
    query: String,
    total_matches: usize,
    sessions: Vec<SearchResultWithContext>,
}

/// Parses a date filter string into a DateTime.
///
/// Supports:
/// - Relative formats: "7d" (7 days), "2w" (2 weeks), "1m" (1 month)
/// - Absolute format: "2024-01-15" (ISO date)
fn parse_date(date_str: &str) -> Result<chrono::DateTime<Utc>> {
    let date_str = date_str.trim().to_lowercase();

    // Try relative format first (e.g., "7d", "2w", "1m")
    if date_str.ends_with('d') {
        let days: i64 = date_str[..date_str.len() - 1]
            .parse()
            .context("Invalid number of days")?;
        return Ok(Utc::now() - Duration::days(days));
    }

    if date_str.ends_with('w') {
        let weeks: i64 = date_str[..date_str.len() - 1]
            .parse()
            .context("Invalid number of weeks")?;
        return Ok(Utc::now() - Duration::weeks(weeks));
    }

    if date_str.ends_with('m') {
        let months: i64 = date_str[..date_str.len() - 1]
            .parse()
            .context("Invalid number of months")?;
        // Approximate months as 30 days
        return Ok(Utc::now() - Duration::days(months * 30));
    }

    // Try absolute format (YYYY-MM-DD)
    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .context("Invalid date format. Use YYYY-MM-DD or relative format like 7d, 2w, 1m")?;

    // Midnight is always a valid time, so this should never fail
    let datetime = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow::anyhow!("Failed to create datetime from date {date_str}"))?;

    Ok(datetime.and_utc())
}

/// Formats a relative time string for display (e.g., "2 hours ago").
fn format_relative_time(dt: &chrono::DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    if duration.num_minutes() < 1 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        let mins = duration.num_minutes();
        format!("{} minute{} ago", mins, if mins == 1 { "" } else { "s" })
    } else if duration.num_hours() < 24 {
        let hours = duration.num_hours();
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else if duration.num_days() < 7 {
        let days = duration.num_days();
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    } else if duration.num_weeks() < 4 {
        let weeks = duration.num_weeks();
        format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" })
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

/// Extracts the project name from a working directory path.
fn extract_project_name(working_directory: &str) -> &str {
    working_directory
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(working_directory)
}

/// Truncates a string to a maximum length, adding ellipsis if needed.
fn truncate_content(content: &str, max_len: usize) -> String {
    let content = content.trim().replace('\n', " ");
    if content.len() <= max_len {
        content
    } else {
        format!("{}...", &content.chars().take(max_len - 3).collect::<String>())
    }
}

/// Formats a role for display.
fn format_role(role: &MessageRole) -> colored::ColoredString {
    match role {
        MessageRole::User => "user".blue(),
        MessageRole::Assistant => "assistant".green(),
        MessageRole::System => "system".yellow(),
    }
}

/// Groups search results by session and adds context messages.
fn group_results_with_context(
    db: &Database,
    results: Vec<SearchResult>,
    context_count: usize,
) -> Result<Vec<SearchResultWithContext>> {
    // Group results by session
    let mut session_groups: HashMap<Uuid, Vec<SearchResult>> = HashMap::new();
    for result in results {
        session_groups
            .entry(result.session_id)
            .or_default()
            .push(result);
    }

    let mut grouped_results = Vec::new();

    for (session_id, session_results) in session_groups {
        // Use the first result to get session metadata
        let first = &session_results[0];
        let project = extract_project_name(&first.working_directory).to_string();

        let mut matches = Vec::new();

        for result in &session_results {
            // Get context messages if requested
            let (before_msgs, after_msgs) = if context_count > 0 {
                db.get_context_messages(&session_id, result.message_index, context_count)?
            } else {
                (Vec::new(), Vec::new())
            };

            // Convert before messages to ContextMessage
            let before: Vec<ContextMessage> = before_msgs
                .into_iter()
                .map(|m| ContextMessage {
                    id: m.id,
                    role: m.role,
                    content: truncate_content(&m.content.text(), 200),
                    index: m.index,
                    is_match: false,
                })
                .collect();

            // Convert after messages to ContextMessage
            let after: Vec<ContextMessage> = after_msgs
                .into_iter()
                .map(|m| ContextMessage {
                    id: m.id,
                    role: m.role,
                    content: truncate_content(&m.content.text(), 200),
                    index: m.index,
                    is_match: false,
                })
                .collect();

            // Create the match with context
            let match_with_context = MatchWithContext {
                message: ContextMessage {
                    id: result.message_id,
                    role: result.role.clone(),
                    content: result.snippet.clone(),
                    index: result.message_index,
                    is_match: true,
                },
                before,
                after,
            };

            matches.push(match_with_context);
        }

        grouped_results.push(SearchResultWithContext {
            session_id,
            tool: first.tool.clone(),
            project,
            working_directory: first.working_directory.clone(),
            git_branch: first.git_branch.clone(),
            session_started_at: first.session_started_at.unwrap_or_else(Utc::now),
            session_message_count: first.session_message_count,
            matches,
        });
    }

    // Sort by session start time (most recent first)
    grouped_results.sort_by(|a, b| b.session_started_at.cmp(&a.session_started_at));

    Ok(grouped_results)
}

/// Displays search results with context in text format.
fn display_results_with_context(
    query: &str,
    grouped_results: &[SearchResultWithContext],
    total_matches: usize,
) {
    if grouped_results.is_empty() {
        println!("{}", format!("No results found for \"{query}\"").dimmed());
        return;
    }

    // Header with match count
    let session_word = if grouped_results.len() == 1 {
        "session"
    } else {
        "sessions"
    };
    println!(
        "Found {} match{} in {} {}\n",
        total_matches.to_string().bold(),
        if total_matches == 1 { "" } else { "es" },
        grouped_results.len(),
        session_word
    );

    for session in grouped_results {
        // Session header with separator
        let session_prefix = &session.session_id.to_string()[..8];
        println!(
            "{} {} {}",
            "---".dimmed(),
            format!("Session {session_prefix}").cyan().bold(),
            "---".dimmed()
        );

        // Session metadata line
        let relative_time = format_relative_time(&session.session_started_at);
        let branch_info = session
            .git_branch
            .as_ref()
            .map(|b| format!(" | Branch: {}", b.white()))
            .unwrap_or_default();

        println!(
            "Tool: {} | Project: {}{}",
            session.tool.yellow(),
            session.project.white(),
            branch_info
        );
        println!(
            "{} | {} messages\n",
            relative_time.dimmed(),
            session.session_message_count
        );

        // Display each match with context
        for match_ctx in &session.matches {
            // Before context (dimmed)
            if !match_ctx.before.is_empty() {
                println!(
                    "  {} {} message{} before {}",
                    "...".dimmed(),
                    match_ctx.before.len(),
                    if match_ctx.before.len() == 1 { "" } else { "s" },
                    "...".dimmed()
                );
                for ctx_msg in &match_ctx.before {
                    let content = truncate_content(&ctx_msg.content, 100);
                    println!(
                        "  [{}] {}",
                        format_role(&ctx_msg.role),
                        content.dimmed()
                    );
                }
                println!();
            }

            // The matching message (highlighted)
            let role_str = format_role(&match_ctx.message.role);
            // Clean up FTS5 highlight markers
            let content = match_ctx.message.content.replace("**", "");
            println!(
                "  [{}] {}    {} {}",
                role_str,
                content.white().bold(),
                "<-".yellow(),
                "match".yellow()
            );

            // After context (dimmed)
            if !match_ctx.after.is_empty() {
                println!();
                for ctx_msg in &match_ctx.after {
                    let content = truncate_content(&ctx_msg.content, 100);
                    println!(
                        "  [{}] {}",
                        format_role(&ctx_msg.role),
                        content.dimmed()
                    );
                }
                println!(
                    "  {} {} message{} after {}",
                    "...".dimmed(),
                    match_ctx.after.len(),
                    if match_ctx.after.len() == 1 { "" } else { "s" },
                    "...".dimmed()
                );
            }

            println!();
        }
    }
}

/// Executes the search command.
///
/// Searches message content and session metadata using FTS5 full-text search,
/// with optional filters for repository, date range, tool, project, branch,
/// and message role. Displays results with surrounding context.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Check if search index needs rebuilding
    if db.search_index_needs_rebuild()? {
        println!(
            "{}",
            "Building search index for existing messages...".yellow()
        );
        let count = db.rebuild_search_index()?;
        println!("Indexed {count} messages.\n");
    }

    // Parse date filters
    let since = args.since.as_ref().map(|s| parse_date(s)).transpose()?;
    let until = args.until.as_ref().map(|s| parse_date(s)).transpose()?;

    // Validate role filter
    if let Some(ref role) = args.role {
        let role_lower = role.to_lowercase();
        if role_lower != "user" && role_lower != "assistant" && role_lower != "system" {
            anyhow::bail!("Invalid role '{role}'. Use 'user', 'assistant', or 'system'.");
        }
    }

    // Build search options
    let options = SearchOptions {
        query: args.query.clone(),
        limit: args.limit,
        tool: args.tool.clone(),
        since,
        until,
        project: args.project.clone(),
        branch: args.branch.clone(),
        role: args.role.clone(),
        repo: args.repo.clone(),
        context: args.context,
    };

    // Execute the search
    let results = db.search_with_options(&options)?;
    let total_matches = results.len();

    match args.format {
        OutputFormat::Json => {
            // Group results and add context for JSON output
            let grouped = group_results_with_context(&db, results, args.context)?;
            let output = SearchOutputWithContext {
                query: args.query,
                total_matches,
                sessions: grouped,
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            if results.is_empty() {
                println!(
                    "{}",
                    format!("No results found for \"{}\"", args.query).dimmed()
                );

                // Provide helpful suggestions
                let has_filters = args.repo.is_some()
                    || args.since.is_some()
                    || args.until.is_some()
                    || args.role.is_some()
                    || args.tool.is_some()
                    || args.project.is_some()
                    || args.branch.is_some();

                if has_filters {
                    println!(
                        "{}",
                        "Try removing filters to broaden your search.".dimmed()
                    );
                }
                return Ok(());
            }

            // Group results and add context
            let grouped = group_results_with_context(&db, results, args.context)?;
            display_results_with_context(&args.query, &grouped, total_matches);

            // Display summary
            if total_matches >= args.limit {
                println!(
                    "{}",
                    format!(
                        "Showing first {} results. Use --limit to see more.",
                        args.limit
                    )
                    .dimmed()
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_date_days() {
        let result = parse_date("7d").expect("Should parse 7d");
        let expected = Utc::now() - Duration::days(7);
        // Allow 1 second of tolerance for test execution time
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_weeks() {
        let result = parse_date("2w").expect("Should parse 2w");
        let expected = Utc::now() - Duration::weeks(2);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_months() {
        let result = parse_date("1m").expect("Should parse 1m");
        let expected = Utc::now() - Duration::days(30);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_absolute() {
        let result = parse_date("2024-01-15").expect("Should parse date");
        assert_eq!(result.format("%Y-%m-%d").to_string(), "2024-01-15");
    }

    #[test]
    fn test_parse_date_invalid() {
        assert!(parse_date("invalid").is_err());
        assert!(parse_date("abc123").is_err());
    }

    #[test]
    fn test_format_relative_time_minutes() {
        let dt = Utc::now() - Duration::minutes(5);
        let result = format_relative_time(&dt);
        assert!(result.contains("minute"));
    }

    #[test]
    fn test_format_relative_time_hours() {
        let dt = Utc::now() - Duration::hours(3);
        let result = format_relative_time(&dt);
        assert!(result.contains("hour"));
    }

    #[test]
    fn test_format_relative_time_days() {
        let dt = Utc::now() - Duration::days(2);
        let result = format_relative_time(&dt);
        assert!(result.contains("day"));
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(extract_project_name("/home/user/projects/lore"), "lore");
        assert_eq!(extract_project_name("/Users/dev/my-project"), "my-project");
        assert_eq!(extract_project_name("simple"), "simple");
    }

    #[test]
    fn test_truncate_content_short() {
        let content = "Hello world";
        assert_eq!(truncate_content(content, 50), "Hello world");
    }

    #[test]
    fn test_truncate_content_long() {
        let content = "This is a very long string that should be truncated";
        let result = truncate_content(content, 20);
        assert!(result.len() <= 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_content_newlines() {
        let content = "Hello\nworld\ntest";
        let result = truncate_content(content, 50);
        assert!(!result.contains('\n'));
        assert_eq!(result, "Hello world test");
    }
}

//! Search command - search session content.
//!
//! Provides full-text search across session messages using SQLite FTS5.
//! Supports filtering by repository, date, and message role.

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::storage::db::Database;
use crate::storage::models::SearchResult;

/// Arguments for the search command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore search \"auth\"                  Search for 'auth' in messages\n    \
    lore search \"bug fix\" --limit 20    Show up to 20 results\n    \
    lore search api --since 7d          Last 7 days only\n    \
    lore search error --role assistant  Only AI responses\n    \
    lore search test --repo /path       Filter by repository")]
pub struct Args {
    /// Text to search for in session messages
    #[arg(value_name = "QUERY")]
    #[arg(
        long_help = "The text to search for. Uses SQLite FTS5 full-text search,\n\
        which supports word matching and basic boolean operators.\n\
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

    /// Filter by date (e.g., 7d, 2w, 1m, or 2024-01-01)
    #[arg(long, value_name = "DATE")]
    #[arg(long_help = "Only search sessions from after this date. Accepts:\n\
        - Relative: 7d (days), 2w (weeks), 1m (months)\n\
        - Absolute: 2024-01-15 (ISO date format)")]
    pub since: Option<String>,

    /// Filter by message role (user, assistant, system)
    #[arg(long, value_name = "ROLE")]
    #[arg(long_help = "Only search messages from a specific role:\n\
        - user: human messages\n\
        - assistant: AI responses\n\
        - system: system prompts")]
    pub role: Option<String>,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for search results.
#[derive(Serialize)]
struct SearchOutput {
    query: String,
    total_results: usize,
    results: Vec<SearchResult>,
}

/// Parses a date filter string into a DateTime.
///
/// Supports:
/// - Relative formats: "7d" (7 days), "2w" (2 weeks), "1m" (1 month)
/// - Absolute format: "2024-01-15" (ISO date)
fn parse_since(since_str: &str) -> Result<chrono::DateTime<Utc>> {
    let since_str = since_str.trim().to_lowercase();

    // Try relative format first (e.g., "7d", "2w", "1m")
    if since_str.ends_with('d') {
        let days: i64 = since_str[..since_str.len() - 1]
            .parse()
            .context("Invalid number of days")?;
        return Ok(Utc::now() - Duration::days(days));
    }

    if since_str.ends_with('w') {
        let weeks: i64 = since_str[..since_str.len() - 1]
            .parse()
            .context("Invalid number of weeks")?;
        return Ok(Utc::now() - Duration::weeks(weeks));
    }

    if since_str.ends_with('m') {
        let months: i64 = since_str[..since_str.len() - 1]
            .parse()
            .context("Invalid number of months")?;
        // Approximate months as 30 days
        return Ok(Utc::now() - Duration::days(months * 30));
    }

    // Try absolute format (YYYY-MM-DD)
    let date = NaiveDate::parse_from_str(&since_str, "%Y-%m-%d")
        .context("Invalid date format. Use YYYY-MM-DD or relative format like 7d, 2w, 1m")?;

    // Midnight is always a valid time, so this should never fail
    let datetime = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow::anyhow!("Failed to create datetime from date {since_str}"))?;

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

/// Extracts the repository name from a working directory path.
fn extract_repo_name(working_directory: &str) -> &str {
    working_directory
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(working_directory)
}

/// Executes the search command.
///
/// Searches message content using FTS5 full-text search, with optional
/// filters for repository, date range, and message role.
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

    // Parse the since filter if provided
    let since = args.since.as_ref().map(|s| parse_since(s)).transpose()?;

    // Validate role filter
    if let Some(ref role) = args.role {
        let role_lower = role.to_lowercase();
        if role_lower != "user" && role_lower != "assistant" && role_lower != "system" {
            anyhow::bail!("Invalid role '{role}'. Use 'user', 'assistant', or 'system'.");
        }
    }

    // Execute the search
    let results = db.search_messages(
        &args.query,
        args.limit,
        args.repo.as_deref(),
        since,
        args.role.as_deref(),
    )?;

    match args.format {
        OutputFormat::Json => {
            let output = SearchOutput {
                query: args.query,
                total_results: results.len(),
                results,
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
                if args.repo.is_some() || args.since.is_some() || args.role.is_some() {
                    println!(
                        "{}",
                        "Try removing filters to broaden your search.".dimmed()
                    );
                }
                return Ok(());
            }

            // Display header
            println!("Search results for \"{}\":\n", args.query.cyan());

            // Display results
            for (idx, result) in results.iter().enumerate() {
                let session_prefix = &result.session_id.to_string()[..8];
                let relative_time = format_relative_time(&result.timestamp);
                let repo_name = extract_repo_name(&result.working_directory);

                let role_display = match result.role {
                    crate::storage::MessageRole::User => "User".blue(),
                    crate::storage::MessageRole::Assistant => "Assistant".green(),
                    crate::storage::MessageRole::System => "System".yellow(),
                };

                println!(
                    "[{}] Session {} - {} - {}",
                    (idx + 1).to_string().bold(),
                    session_prefix.cyan(),
                    relative_time.dimmed(),
                    repo_name.white()
                );

                // Format the snippet - replace ** markers with colored text
                let snippet_formatted = result.snippet.replace("**", ""); // FTS5 markers

                println!("    [{role_display}] ...{snippet_formatted}...\n");
            }

            // Display count info
            let count_msg = if results.len() >= args.limit {
                format!(
                    "Found {} results (showing 1-{}, use --limit to see more)",
                    results.len(),
                    args.limit
                )
            } else {
                format!(
                    "Found {} result{}",
                    results.len(),
                    if results.len() == 1 { "" } else { "s" }
                )
            };
            println!("{}", count_msg.dimmed());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_since_days() {
        let result = parse_since("7d").expect("Should parse 7d");
        let expected = Utc::now() - Duration::days(7);
        // Allow 1 second of tolerance for test execution time
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_since_weeks() {
        let result = parse_since("2w").expect("Should parse 2w");
        let expected = Utc::now() - Duration::weeks(2);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_since_months() {
        let result = parse_since("1m").expect("Should parse 1m");
        let expected = Utc::now() - Duration::days(30);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_since_absolute_date() {
        let result = parse_since("2024-01-15").expect("Should parse date");
        assert_eq!(result.format("%Y-%m-%d").to_string(), "2024-01-15");
    }

    #[test]
    fn test_parse_since_invalid() {
        assert!(parse_since("invalid").is_err());
        assert!(parse_since("abc123").is_err());
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
    fn test_extract_repo_name() {
        assert_eq!(extract_repo_name("/home/user/projects/lore"), "lore");
        assert_eq!(extract_repo_name("/Users/dev/my-project"), "my-project");
        assert_eq!(extract_repo_name("simple"), "simple");
    }
}

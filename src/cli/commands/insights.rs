//! Insights command - AI development analytics.
//!
//! Surfaces interesting analytics about AI-assisted development patterns
//! including coverage, tool usage, activity patterns, and code impact.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::cli::OutputFormat;
use crate::git;
use crate::storage::db::Database;
use crate::storage::models::extract_session_files;

/// Arguments for the insights command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore insights                     Show AI development insights\n    \
    lore insights --since 30d         Last 30 days only\n    \
    lore insights --since 2025-01-01  Since a specific date\n    \
    lore insights --repo /path/to/repo  Scope to specific repo\n    \
    lore insights --format json       Machine-readable output")]
pub struct Args {
    /// Scope insights to a specific repository path
    #[arg(long, value_name = "PATH")]
    #[arg(
        long_help = "Only include sessions from repositories matching this path\n\
        prefix. Defaults to the current working directory."
    )]
    pub repo: Option<String>,

    /// Start date filter (e.g., "30d", "3m", "2025-01-01")
    #[arg(long, value_name = "DATE")]
    #[arg(long_help = "Only include sessions after this date. Accepts:\n\
        - Relative: 7d (days), 2w (weeks), 1m (months)\n\
        - Absolute: 2025-01-15 (ISO date format)\n\
        Defaults to all time if not specified.")]
    pub since: Option<String>,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for insights.
#[derive(Serialize)]
struct InsightsOutput {
    period: PeriodInfo,
    coverage: CoverageInfo,
    tools: Vec<ToolInfo>,
    activity: ActivityInfo,
    top_files: Vec<FileInfo>,
}

#[derive(Serialize)]
struct PeriodInfo {
    since: Option<String>,
    until: String,
    description: String,
}

#[derive(Serialize)]
struct CoverageInfo {
    total_commits: usize,
    linked_commits: usize,
    coverage_percent: f64,
}

#[derive(Serialize)]
struct ToolInfo {
    name: String,
    sessions: i32,
    percent: f64,
}

#[derive(Serialize)]
struct ActivityInfo {
    total_sessions: usize,
    total_messages: i32,
    avg_duration_minutes: Option<f64>,
    avg_messages_per_session: Option<f64>,
    most_active_day: Option<String>,
}

#[derive(Serialize)]
struct FileInfo {
    path: String,
    session_count: usize,
}

/// Parses a date filter string into a DateTime.
///
/// Supports relative formats (7d, 2w, 1m) and absolute (2025-01-15).
fn parse_date(date_str: &str) -> Result<DateTime<Utc>> {
    let date_str = date_str.trim().to_lowercase();

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
        return Ok(Utc::now() - Duration::days(months * 30));
    }

    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .context("Invalid date format. Use YYYY-MM-DD or relative format like 7d, 2w, 1m")?;

    let datetime = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow::anyhow!("Failed to create datetime from date {date_str}"))?;

    Ok(datetime.and_utc())
}

/// Converts a SQLite weekday number (0=Sunday) to a day name.
fn weekday_name(day: i32) -> &'static str {
    match day {
        0 => "Sundays",
        1 => "Mondays",
        2 => "Tuesdays",
        3 => "Wednesdays",
        4 => "Thursdays",
        5 => "Fridays",
        6 => "Saturdays",
        _ => "Unknown",
    }
}

/// Builds a human-readable period description for the header.
fn period_description(since: Option<&DateTime<Utc>>) -> String {
    match since {
        Some(dt) => {
            let days = (Utc::now() - *dt).num_days();
            if days <= 7 {
                "last 7 days".to_string()
            } else if days <= 30 {
                format!("last {} days", days)
            } else if days <= 90 {
                let months = days / 30;
                format!(
                    "last {} month{}",
                    months,
                    if months == 1 { "" } else { "s" }
                )
            } else {
                format!("since {}", dt.format("%Y-%m-%d"))
            }
        }
        None => "all time".to_string(),
    }
}

/// Gathers top files across sessions by loading messages and extracting file paths.
fn gather_top_files(
    db: &Database,
    sessions: &[crate::storage::models::Session],
    top_n: usize,
) -> Result<Vec<(String, usize)>> {
    let mut file_counts: HashMap<String, usize> = HashMap::new();

    for session in sessions {
        let messages = db.get_messages(&session.id)?;
        let files = extract_session_files(&messages, &session.working_directory);
        // Count each file once per session (not once per mention)
        let unique_files: std::collections::HashSet<String> = files.into_iter().collect();
        for file in unique_files {
            *file_counts.entry(file).or_insert(0) += 1;
        }
    }

    let mut sorted: Vec<(String, usize)> = file_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(top_n);
    Ok(sorted)
}

/// Executes the insights command.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Parse the --since filter
    let since = args.since.as_ref().map(|s| parse_date(s)).transpose()?;

    // Determine working directory filter
    let working_dir = match &args.repo {
        Some(repo) => Some(repo.clone()),
        None => std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
    };
    let working_dir_ref = working_dir.as_deref();

    // Gather data
    let sessions = db.sessions_in_date_range(since, None, working_dir_ref)?;
    let total_sessions = sessions.len();

    if total_sessions == 0 {
        println!("{}", "No sessions found for the given filters.".dimmed());
        println!(
            "{}",
            "Try removing --repo or --since filters, or import sessions first with 'lore import'."
                .dimmed()
        );
        return Ok(());
    }

    // Total messages across matching sessions
    let total_messages: i32 = sessions.iter().map(|s| s.message_count).sum();

    // Tool breakdown
    let tools_breakdown = db.sessions_by_tool_in_range(since, working_dir_ref)?;

    // Activity stats
    let avg_duration = db.average_session_duration_minutes(since, working_dir_ref)?;
    let avg_messages = db.average_message_count(since, working_dir_ref)?;
    let weekday_counts = db.sessions_by_weekday(since, working_dir_ref)?;

    // Find the most active weekday
    let most_active_day = weekday_counts
        .iter()
        .max_by_key(|(_day, count)| *count)
        .map(|(day, _count)| weekday_name(*day));

    // Coverage: count commits in repo vs linked commits
    let (total_commits, linked_commits) = calculate_coverage(since, working_dir_ref, &db)?;

    // Top files (limit to 10)
    let top_files = gather_top_files(&db, &sessions, 10)?;

    let period_desc = period_description(since.as_ref());

    match args.format {
        OutputFormat::Json => {
            let output = InsightsOutput {
                period: PeriodInfo {
                    since: since.map(|dt| dt.to_rfc3339()),
                    until: Utc::now().to_rfc3339(),
                    description: period_desc,
                },
                coverage: CoverageInfo {
                    total_commits,
                    linked_commits,
                    coverage_percent: if total_commits > 0 {
                        (linked_commits as f64 / total_commits as f64) * 100.0
                    } else {
                        0.0
                    },
                },
                tools: tools_breakdown
                    .iter()
                    .map(|(name, count)| ToolInfo {
                        name: name.clone(),
                        sessions: *count,
                        percent: if total_sessions > 0 {
                            (*count as f64 / total_sessions as f64) * 100.0
                        } else {
                            0.0
                        },
                    })
                    .collect(),
                activity: ActivityInfo {
                    total_sessions,
                    total_messages,
                    avg_duration_minutes: avg_duration,
                    avg_messages_per_session: avg_messages,
                    most_active_day: most_active_day.map(|s| s.to_string()),
                },
                top_files: top_files
                    .iter()
                    .map(|(path, count)| FileInfo {
                        path: path.clone(),
                        session_count: *count,
                    })
                    .collect(),
            };
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            display_text(&DisplayData {
                period_desc: &period_desc,
                total_commits,
                linked_commits,
                tools_breakdown: &tools_breakdown,
                total_sessions,
                total_messages,
                avg_duration,
                avg_messages,
                most_active_day,
                top_files: &top_files,
            });
        }
    }

    Ok(())
}

/// Calculates commit coverage (total commits vs commits with linked sessions).
///
/// Gets the list of commits from the git repo in the time range, then checks
/// how many of those specific commits have at least one session link.
fn calculate_coverage(
    since: Option<DateTime<Utc>>,
    working_dir: Option<&str>,
    db: &Database,
) -> Result<(usize, usize)> {
    let repo_path = working_dir.map(Path::new).unwrap_or_else(|| Path::new("."));

    let after = since.unwrap_or_else(|| {
        // Default to 1 year ago if no since filter
        Utc::now() - Duration::days(365)
    });
    let before = Utc::now();

    let commits = git::get_commits_in_time_range(repo_path, after, before).unwrap_or_default();
    let total_commits = commits.len();

    // Count how many of those commits have at least one linked session
    let mut linked_count = 0;
    for commit in &commits {
        let links = db.get_links_by_commit(&commit.sha)?;
        if !links.is_empty() {
            linked_count += 1;
        }
    }

    Ok((total_commits, linked_count))
}

/// Collected data for text display rendering.
struct DisplayData<'a> {
    period_desc: &'a str,
    total_commits: usize,
    linked_commits: usize,
    tools_breakdown: &'a [(String, i32)],
    total_sessions: usize,
    total_messages: i32,
    avg_duration: Option<f64>,
    avg_messages: Option<f64>,
    most_active_day: Option<&'a str>,
    top_files: &'a [(String, usize)],
}

/// Displays insights in text format.
fn display_text(data: &DisplayData<'_>) {
    let period_desc = data.period_desc;
    let total_commits = data.total_commits;
    let linked_commits = data.linked_commits;
    let tools_breakdown = data.tools_breakdown;
    let total_sessions = data.total_sessions;
    let total_messages = data.total_messages;
    let avg_duration = data.avg_duration;
    let avg_messages = data.avg_messages;
    let most_active_day = data.most_active_day;
    let top_files = data.top_files;
    // Header
    let header = format!("AI Development Insights ({})", period_desc);
    println!("{}", header.bold());
    println!("{}", "=".repeat(header.len()));
    println!();

    // Coverage section
    println!("{}", "Coverage".bold());
    if total_commits > 0 {
        let coverage = (linked_commits as f64 / total_commits as f64) * 100.0;
        println!(
            "  {} total, {} with linked sessions ({})",
            format!("{}", total_commits).cyan(),
            format!("{}", linked_commits).cyan(),
            format!("{:.0}%", coverage).green()
        );
    } else {
        println!(
            "  {}",
            "No commits found in the time range (not in a git repo?)".dimmed()
        );
    }
    println!();

    // Tools section
    if !tools_breakdown.is_empty() {
        println!("{}", "Tools".bold());
        for (tool, count) in tools_breakdown {
            let pct = if total_sessions > 0 {
                (*count as f64 / total_sessions as f64) * 100.0
            } else {
                0.0
            };
            println!(
                "  {:<20} {} ({})",
                tool,
                format!("{} sessions", count).dimmed(),
                format!("{:.0}%", pct).dimmed()
            );
        }
        println!();
    }

    // Activity section
    println!("{}", "Activity".bold());
    println!(
        "  {}      {} total",
        "Sessions:".dimmed(),
        format!("{}", total_sessions).cyan()
    );
    println!(
        "  {}      {} total",
        "Messages:".dimmed(),
        format!("{}", total_messages).cyan()
    );
    if let Some(avg_dur) = avg_duration {
        println!(
            "  {}  {} min",
            "Avg duration:".dimmed(),
            format!("{:.0}", avg_dur).cyan()
        );
    }
    if let Some(avg_msg) = avg_messages {
        println!(
            "  {}  {} per session",
            "Avg messages:".dimmed(),
            format!("{:.0}", avg_msg).cyan()
        );
    }
    if let Some(day) = most_active_day {
        println!("  {} {}", "Most active:".dimmed(), day.cyan());
    }
    println!();

    // Top Files section
    if !top_files.is_empty() {
        println!("{}", "Top Files".bold());
        for (path, count) in top_files {
            let session_word = if *count == 1 { "session" } else { "sessions" };
            println!(
                "  {:<40} {} {}",
                path,
                format!("{}", count).dimmed(),
                session_word.dimmed()
            );
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_date_days() {
        let result = parse_date("30d").unwrap();
        let expected = Utc::now() - Duration::days(30);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_weeks() {
        let result = parse_date("2w").unwrap();
        let expected = Utc::now() - Duration::weeks(2);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_months() {
        let result = parse_date("3m").unwrap();
        let expected = Utc::now() - Duration::days(90);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_absolute() {
        let result = parse_date("2025-06-15").unwrap();
        assert_eq!(result.format("%Y-%m-%d").to_string(), "2025-06-15");
    }

    #[test]
    fn test_parse_date_invalid() {
        assert!(parse_date("invalid").is_err());
    }

    #[test]
    fn test_weekday_name() {
        assert_eq!(weekday_name(0), "Sundays");
        assert_eq!(weekday_name(1), "Mondays");
        assert_eq!(weekday_name(2), "Tuesdays");
        assert_eq!(weekday_name(3), "Wednesdays");
        assert_eq!(weekday_name(4), "Thursdays");
        assert_eq!(weekday_name(5), "Fridays");
        assert_eq!(weekday_name(6), "Saturdays");
        assert_eq!(weekday_name(7), "Unknown");
    }

    #[test]
    fn test_period_description_none() {
        assert_eq!(period_description(None), "all time");
    }

    #[test]
    fn test_period_description_recent() {
        let dt = Utc::now() - Duration::days(5);
        assert_eq!(period_description(Some(&dt)), "last 7 days");
    }

    #[test]
    fn test_period_description_month() {
        let dt = Utc::now() - Duration::days(25);
        assert_eq!(period_description(Some(&dt)), "last 25 days");
    }

    #[test]
    fn test_period_description_months() {
        let dt = Utc::now() - Duration::days(60);
        assert_eq!(period_description(Some(&dt)), "last 2 months");
    }

    #[test]
    fn test_period_description_old() {
        let dt = Utc::now() - Duration::days(200);
        let result = period_description(Some(&dt));
        assert!(result.starts_with("since "));
    }
}

//! Sessions command - list and filter sessions.
//!
//! Displays a list of imported sessions with filtering options.
//! Sessions can be filtered by working directory and output in
//! text, JSON, or markdown format.

use anyhow::Result;
use colored::Colorize;

use crate::cli::OutputFormat;
use crate::storage::Database;

/// Arguments for the sessions command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore sessions                  List recent sessions (default 20)\n    \
    lore sessions --limit 50       Show up to 50 sessions\n    \
    lore sessions --repo .         Filter to current directory\n    \
    lore sessions --repo /path     Filter to specific path\n    \
    lore sessions --tag bug-fix    Filter to sessions with 'bug-fix' tag\n    \
    lore sessions --format json    Output as JSON")]
pub struct Args {
    /// Filter to sessions in this directory (prefix match)
    #[arg(short, long, value_name = "PATH")]
    #[arg(
        long_help = "Filter sessions to those with a working directory matching\n\
        this path prefix. Use '.' for the current directory."
    )]
    pub repo: Option<String>,

    /// Filter to sessions with this tag
    #[arg(short, long, value_name = "LABEL")]
    pub tag: Option<String>,

    /// Maximum number of sessions to display
    #[arg(short, long, default_value = "20", value_name = "N")]
    pub limit: usize,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// Executes the sessions command.
///
/// Lists sessions from the database, optionally filtered by
/// working directory prefix or tag.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Resolve repo path if provided
    let working_dir = args.repo.map(|r| {
        if r == "." {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| r)
        } else {
            r
        }
    });

    // Get sessions - either filtered by tag or by normal query
    let sessions = if let Some(ref tag_label) = args.tag {
        let mut tagged_sessions = db.list_sessions_with_tag(tag_label, args.limit)?;
        // If repo filter is also specified, filter further
        if let Some(ref wd) = working_dir {
            tagged_sessions.retain(|s| s.working_directory.starts_with(wd));
        }
        tagged_sessions
    } else {
        db.list_sessions(args.limit, working_dir.as_deref())?
    };

    if sessions.is_empty() {
        println!("{}", "No sessions found.".dimmed());
        println!();
        println!("Run 'lore import' to import sessions from Claude Code.");
        return Ok(());
    }

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&sessions)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            // Column widths for consistent alignment
            const ID_WIDTH: usize = 12;
            const STARTED_WIDTH: usize = 16;
            const MESSAGES_WIDTH: usize = 8;
            const BRANCH_WIDTH: usize = 24;

            println!(
                "{}",
                format!(
                    "{:<ID_WIDTH$}  {:<STARTED_WIDTH$}  {:>MESSAGES_WIDTH$}  {:<BRANCH_WIDTH$}  {}",
                    "ID", "STARTED", "MESSAGES", "BRANCH", "DIRECTORY"
                )
                .bold()
            );

            for session in &sessions {
                let id_short = &session.id.to_string()[..8];
                let has_summary = db.get_summary(&session.id).ok().flatten().is_some();
                let id_display = if has_summary {
                    format!("{} {}", id_short.cyan(), "[S]".green())
                } else {
                    format!("{}", id_short.cyan())
                };
                let started = session.started_at.format("%Y-%m-%d %H:%M").to_string();
                let branch_history = db.get_session_branch_history(session.id)?;
                let branch_display = format_branch_history(&branch_history, BRANCH_WIDTH);
                let dir = session
                    .working_directory
                    .split('/')
                    .next_back()
                    .unwrap_or(&session.working_directory);

                println!(
                    "{:<ID_WIDTH$}  {:<STARTED_WIDTH$}  {:>MESSAGES_WIDTH$}  {:<BRANCH_WIDTH$}  {}",
                    id_display,
                    started.dimmed(),
                    session.message_count,
                    branch_display.yellow(),
                    dir
                );
            }
        }
    }

    Ok(())
}

/// Truncates a string to fit within a maximum width.
///
/// If the string is longer than `max_width`, it is truncated and "..." is appended.
/// The total length of the returned string will be at most `max_width` characters.
///
/// Branch names are typically ASCII, so we use byte-based slicing for simplicity.
/// If non-ASCII characters are present, this may produce unexpected results,
/// but branch names rarely contain non-ASCII characters.
fn truncate_to_width(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else if max_width <= 3 {
        ".".repeat(max_width)
    } else {
        format!("{}...", &s[..max_width - 3])
    }
}

/// Formats a branch history for display within a maximum width.
///
/// Joins branches with arrows. If there are more than 3 branches,
/// truncates to show: first -> second -> ... -> last
///
/// The result is truncated to fit within `max_width` characters.
/// Returns "-" if the history is empty.
fn format_branch_history(branches: &[String], max_width: usize) -> String {
    let result = match branches.len() {
        0 => "-".to_string(),
        1 => branches[0].clone(),
        2 | 3 => branches.join(" -> "),
        _ => {
            // More than 3 branches: show first -> second -> ... -> last
            format!(
                "{} -> {} -> ... -> {}",
                branches[0],
                branches[1],
                branches.last().unwrap()
            )
        }
    };

    truncate_to_width(&result, max_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for truncate_to_width

    #[test]
    fn test_truncate_to_width_short_string() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_to_width_exact_length() {
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_to_width_needs_truncation() {
        assert_eq!(truncate_to_width("hello world", 8), "hello...");
    }

    #[test]
    fn test_truncate_to_width_very_small() {
        assert_eq!(truncate_to_width("hello", 3), "...");
        assert_eq!(truncate_to_width("hello", 2), "..");
        assert_eq!(truncate_to_width("hello", 1), ".");
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn test_truncate_to_width_minimum_for_ellipsis() {
        // 4 chars allows 1 char + "..."
        assert_eq!(truncate_to_width("hello", 4), "h...");
    }

    // Tests for format_branch_history

    #[test]
    fn test_format_branch_history_empty() {
        let branches: Vec<String> = vec![];
        assert_eq!(format_branch_history(&branches, 24), "-");
    }

    #[test]
    fn test_format_branch_history_single() {
        let branches = vec!["main".to_string()];
        assert_eq!(format_branch_history(&branches, 24), "main");
    }

    #[test]
    fn test_format_branch_history_two() {
        let branches = vec!["main".to_string(), "feat/auth".to_string()];
        assert_eq!(format_branch_history(&branches, 24), "main -> feat/auth");
    }

    #[test]
    fn test_format_branch_history_three() {
        let branches = vec![
            "main".to_string(),
            "feat/auth".to_string(),
            "main".to_string(),
        ];
        assert_eq!(
            format_branch_history(&branches, 30),
            "main -> feat/auth -> main"
        );
    }

    #[test]
    fn test_format_branch_history_many_branches() {
        let branches = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "feat/b".to_string(),
            "feat/c".to_string(),
            "main".to_string(),
        ];
        assert_eq!(
            format_branch_history(&branches, 50),
            "main -> feat/a -> ... -> main"
        );
    }

    #[test]
    fn test_format_branch_history_truncates_long_result() {
        // Long branch names that will exceed the width
        let branches = vec![
            "main".to_string(),
            "feat/phase-6-configuration-ux".to_string(),
            "main".to_string(),
        ];
        // "main -> feat/phase-6-configuration-ux -> main" = 46 chars
        let result = format_branch_history(&branches, 24);
        assert_eq!(result.len(), 24);
        assert_eq!(result, "main -> feat/phase-6-...");
    }

    #[test]
    fn test_format_branch_history_single_long_branch() {
        let branches = vec!["feat/very-long-branch-name-here".to_string()];
        let result = format_branch_history(&branches, 20);
        assert_eq!(result.len(), 20);
        assert_eq!(result, "feat/very-long-br...");
    }

    #[test]
    fn test_format_branch_history_fits_exactly() {
        let branches = vec!["main".to_string(), "dev".to_string()];
        // "main -> dev" = 11 chars
        let result = format_branch_history(&branches, 11);
        assert_eq!(result, "main -> dev");
    }
}

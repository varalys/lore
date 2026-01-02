//! Database management command - vacuum, prune, and stats.
//!
//! Provides subcommands for managing the Lore database including:
//! - vacuum: Reclaim disk space
//! - prune: Delete old sessions
//! - stats: Show database statistics

use std::io::{self, Write};

use anyhow::{bail, Result};
use chrono::{Duration, Utc};
use colored::Colorize;

use crate::storage::Database;

/// Arguments for the db command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore db stats                   Show database statistics\n    \
    lore db vacuum                  Reclaim unused space\n    \
    lore db prune --older-than 90d  Delete sessions older than 90 days\n    \
    lore db prune --older-than 6m --dry-run  Preview what would be deleted")]
pub struct Args {
    /// Database subcommand to run
    #[command(subcommand)]
    pub command: DbCommand,
}

/// Database management subcommands.
#[derive(clap::Subcommand)]
pub enum DbCommand {
    /// Reclaim unused disk space by running SQLite VACUUM
    #[command(
        long_about = "Runs SQLite VACUUM to rebuild the database file and reclaim\n\
        unused space. This can take some time on large databases and\n\
        temporarily uses extra disk space during the operation."
    )]
    Vacuum,

    /// Delete sessions older than a specified duration
    #[command(
        long_about = "Deletes sessions older than the specified duration along with\n\
        all their messages and links. Use --dry-run to preview what\n\
        would be deleted without making changes."
    )]
    Prune(PruneArgs),

    /// Show database statistics
    #[command(
        long_about = "Displays statistics about the Lore database including:\n\
        - Total sessions, messages, and links\n\
        - Database file size\n\
        - Date range of sessions\n\
        - Breakdown by AI tool"
    )]
    Stats,
}

/// Arguments for the prune subcommand.
#[derive(clap::Args)]
pub struct PruneArgs {
    /// Delete sessions older than this duration (e.g., 90d, 6m, 1y)
    #[arg(long, value_name = "DURATION")]
    #[arg(
        long_help = "Duration string specifying how old sessions must be to delete.\n\
        Supported formats:\n  \
        - Nd: N days (e.g., 90d)\n  \
        - Nw: N weeks (e.g., 12w)\n  \
        - Nm: N months (e.g., 6m)\n  \
        - Ny: N years (e.g., 1y)"
    )]
    pub older_than: String,

    /// Show what would be deleted without actually deleting
    #[arg(long)]
    #[arg(long_help = "Preview mode: shows which sessions would be deleted\n\
        without actually removing them from the database.")]
    pub dry_run: bool,

    /// Skip the confirmation prompt
    #[arg(long)]
    #[arg(
        long_help = "Skip the confirmation prompt and proceed with deletion.\n\
        Use with caution as this operation cannot be undone."
    )]
    pub force: bool,
}

/// Executes the db command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        DbCommand::Vacuum => run_vacuum(),
        DbCommand::Prune(prune_args) => run_prune(prune_args),
        DbCommand::Stats => run_stats(),
    }
}

/// Runs the vacuum subcommand.
fn run_vacuum() -> Result<()> {
    let db = Database::open_default()?;

    // Get size before
    let size_before = db.file_size()?.unwrap_or(0);

    println!("{}", "Running VACUUM...".dimmed());

    db.vacuum()?;

    // Get size after
    let size_after = db.file_size()?.unwrap_or(0);

    let saved = size_before.saturating_sub(size_after);

    println!("{} Database vacuumed successfully", "Done.".green().bold());
    println!("  {} {}", "Before:".dimmed(), format_size(size_before));
    println!("  {}  {}", "After:".dimmed(), format_size(size_after));
    if saved > 0 {
        println!("  {}  {}", "Saved:".dimmed(), format_size(saved).green());
    }

    Ok(())
}

/// Runs the prune subcommand.
fn run_prune(args: PruneArgs) -> Result<()> {
    let db = Database::open_default()?;

    // Parse the duration
    let duration = parse_duration(&args.older_than)?;
    let cutoff = Utc::now() - duration;

    // Count sessions that would be deleted
    let count = db.count_sessions_older_than(cutoff)?;

    if count == 0 {
        println!(
            "{}",
            format!("No sessions older than {} found.", args.older_than).dimmed()
        );
        return Ok(());
    }

    // Show what will be deleted
    let cutoff_display = cutoff.format("%Y-%m-%d");
    println!(
        "Found {} {} started before {}",
        count.to_string().yellow(),
        if count == 1 { "session" } else { "sessions" },
        cutoff_display.to_string().cyan()
    );

    if args.dry_run {
        println!();
        println!("{}", "(Dry run - no changes made)".dimmed());
        return Ok(());
    }

    // Confirm unless --force
    if !args.force {
        println!();
        print!(
            "Delete {} {}? [y/N] ",
            count,
            if count == 1 { "session" } else { "sessions" }
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("{}", "Cancelled".dimmed());
            return Ok(());
        }
    }

    // Delete the sessions
    let deleted = db.delete_sessions_older_than(cutoff)?;

    println!(
        "{} {} {}",
        "Deleted".green(),
        deleted,
        if deleted == 1 { "session" } else { "sessions" }
    );

    Ok(())
}

/// Runs the stats subcommand.
fn run_stats() -> Result<()> {
    let db = Database::open_default()?;

    let stats = db.stats()?;
    let file_size = db.file_size()?.unwrap_or(0);

    println!("{}", "Database Statistics".bold());
    println!();
    println!("  {}  {}", "Sessions:".dimmed(), stats.session_count);
    println!("  {}  {}", "Messages:".dimmed(), stats.message_count);
    println!("  {}     {}", "Links:".dimmed(), stats.link_count);
    println!("  {} {}", "File size:".dimmed(), format_size(file_size));

    // Date range
    if let (Some(oldest), Some(newest)) = (stats.oldest_session, stats.newest_session) {
        println!();
        println!("{}", "Date Range".bold());
        println!(
            "  {}   {}",
            "Oldest:".dimmed(),
            oldest.format("%Y-%m-%d %H:%M")
        );
        println!(
            "  {}   {}",
            "Newest:".dimmed(),
            newest.format("%Y-%m-%d %H:%M")
        );
    }

    // Sessions by tool
    if !stats.sessions_by_tool.is_empty() {
        println!();
        println!("{}", "Sessions by Tool".bold());
        for (tool, count) in &stats.sessions_by_tool {
            println!("  {}  {}", format!("{:>14}:", tool).dimmed(), count);
        }
    }

    Ok(())
}

/// Parses a duration string like "90d", "6m", "1y".
///
/// Supported formats:
/// - Nd: N days
/// - Nw: N weeks
/// - Nm: N months (approximated as 30 days each)
/// - Ny: N years (approximated as 365 days each)
fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_lowercase();

    if s.is_empty() {
        bail!("Duration string cannot be empty");
    }

    // Extract the numeric part and the unit
    let unit = s.chars().last().unwrap();
    let number_part = &s[..s.len() - 1];

    let number: i64 = number_part.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid duration '{}'. Expected format: <number><unit> (e.g., 90d, 6m, 1y)",
            s
        )
    })?;

    if number <= 0 {
        bail!("Duration must be a positive number");
    }

    let days = match unit {
        'd' => number,
        'w' => number * 7,
        'm' => number * 30,
        'y' => number * 365,
        _ => bail!(
            "Unknown duration unit '{}'. Supported units: d (days), w (weeks), m (months), y (years)",
            unit
        ),
    };

    Ok(Duration::days(days))
}

/// Formats a file size in bytes as a human-readable string.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_days() {
        let d = parse_duration("90d").unwrap();
        assert_eq!(d.num_days(), 90);
    }

    #[test]
    fn test_parse_duration_weeks() {
        let d = parse_duration("4w").unwrap();
        assert_eq!(d.num_days(), 28);
    }

    #[test]
    fn test_parse_duration_months() {
        let d = parse_duration("6m").unwrap();
        assert_eq!(d.num_days(), 180);
    }

    #[test]
    fn test_parse_duration_years() {
        let d = parse_duration("1y").unwrap();
        assert_eq!(d.num_days(), 365);
    }

    #[test]
    fn test_parse_duration_uppercase() {
        let d = parse_duration("30D").unwrap();
        assert_eq!(d.num_days(), 30);
    }

    #[test]
    fn test_parse_duration_invalid_unit() {
        let result = parse_duration("30x");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_duration_invalid_number() {
        let result = parse_duration("abcd");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_duration_empty() {
        let result = parse_duration("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_duration_negative() {
        let result = parse_duration("-30d");
        assert!(result.is_err());
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(500), "500 bytes");
    }

    #[test]
    fn test_format_size_kb() {
        assert_eq!(format_size(1536), "1.50 KB");
    }

    #[test]
    fn test_format_size_mb() {
        assert_eq!(format_size(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn test_format_size_gb() {
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.00 GB");
    }
}

//! Link command - link sessions to git commits.
//!
//! Creates associations between development sessions and the commits
//! they produced. Links are stored in the database and can be queried
//! by commit SHA to find related sessions.
//!
//! Supports both manual linking (specifying session IDs) and automatic
//! linking based on time proximity and file overlap heuristics.

use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use std::path::Path;
use uuid::Uuid;

use crate::config::Config;
use crate::git::{calculate_link_confidence, get_commit_files, get_commit_info};
use crate::storage::{extract_session_files, Database, LinkCreator, LinkType, SessionLink};

/// Default time window in minutes for finding sessions near a commit.
const DEFAULT_WINDOW_MINUTES: i64 = 30;

/// Arguments for the link command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore link abc123                    Link session to HEAD\n    \
    lore link abc123 def456             Link multiple sessions\n    \
    lore link abc123 --commit 1a2b3c    Link to specific commit\n    \
    lore link --auto                    Auto-link by time/file overlap\n    \
    lore link --auto --threshold 0.8    Require 80% confidence\n    \
    lore link --auto --dry-run          Preview auto-link results")]
pub struct Args {
    /// Session ID prefixes to link (can specify multiple)
    #[arg(value_name = "SESSION")]
    #[arg(
        long_help = "One or more session ID prefixes to link. You only need to\n\
        provide enough characters to uniquely identify each session.\n\
        Required unless --auto is specified."
    )]
    pub sessions: Vec<String>,

    /// Commit to link to (defaults to HEAD)
    #[arg(long, default_value = "HEAD", value_name = "REF")]
    #[arg(
        long_help = "The git commit to link sessions to. Accepts any git reference:\n\
        SHA, HEAD, HEAD~1, branch name, tag, etc. Defaults to HEAD."
    )]
    pub commit: String,

    /// Automatically find and link sessions based on heuristics
    #[arg(long)]
    #[arg(
        long_help = "Automatically find sessions that likely contributed to the\n\
        commit based on time proximity and file overlap. Sessions are\n\
        scored and linked if they meet the confidence threshold."
    )]
    pub auto: bool,

    /// Minimum confidence score (0.0-1.0) for auto-linking
    #[arg(long, value_name = "SCORE")]
    #[arg(long_help = "The minimum confidence score (0.0 to 1.0) required for\n\
        auto-linking. Higher values are more selective. Overrides\n\
        the auto_link_threshold config setting.")]
    pub threshold: Option<f64>,

    /// Preview what would be linked without making changes
    #[arg(long)]
    #[arg(
        long_help = "Shows what links would be created without actually modifying\n\
        the database. Useful for previewing auto-link results."
    )]
    pub dry_run: bool,
}

/// Executes the link command.
///
/// Creates links between the specified sessions and a commit.
/// Uses the current HEAD if no commit is specified.
pub fn run(args: Args) -> Result<()> {
    if args.auto {
        run_auto_link(args)
    } else {
        run_manual_link(args)
    }
}

/// Runs manual linking for explicitly specified session IDs.
fn run_manual_link(args: Args) -> Result<()> {
    if args.sessions.is_empty() {
        anyhow::bail!(
            "No sessions specified. Use --auto for automatic linking or provide session IDs."
        );
    }

    let db = Database::open_default()?;

    // Resolve commit
    let commit_sha = resolve_commit(&args.commit)?;
    let short_sha = &commit_sha[..8.min(commit_sha.len())];
    println!("Linking to commit {}", short_sha.yellow());

    // Find and link each session
    let all_sessions = db.list_sessions(1000, None)?;

    for session_prefix in &args.sessions {
        let session = all_sessions
            .iter()
            .find(|s| s.id.to_string().starts_with(session_prefix));

        let session = match session {
            Some(s) => s,
            None => {
                if all_sessions.is_empty() {
                    anyhow::bail!(
                        "No session found matching '{session_prefix}'. No sessions in database. \
                         Run 'lore import' to import sessions first."
                    );
                } else {
                    anyhow::bail!(
                        "No session found matching '{session_prefix}'. \
                         Run 'lore sessions' to list available sessions."
                    );
                }
            }
        };

        if args.dry_run {
            println!(
                "  {} Would link session {} -> commit {}",
                "[dry-run]".cyan(),
                &session.id.to_string()[..8].cyan(),
                short_sha
            );
            continue;
        }

        let link = SessionLink {
            id: Uuid::new_v4(),
            session_id: session.id,
            link_type: LinkType::Commit,
            commit_sha: Some(commit_sha.clone()),
            branch: None,
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::User,
            confidence: None,
        };

        db.insert_link(&link)?;

        println!(
            "  {} session {} -> commit {}",
            "Linked".green(),
            &session.id.to_string()[..8].cyan(),
            short_sha
        );
    }

    Ok(())
}

/// Runs automatic linking based on heuristics.
fn run_auto_link(args: Args) -> Result<()> {
    let db = Database::open_default()?;
    let config = Config::load()?;

    // Get threshold from args or config
    let threshold = args.threshold.unwrap_or(config.auto_link_threshold);

    // Get commit information
    let cwd = std::env::current_dir()?;
    let commit_info = get_commit_info(&cwd, &args.commit)?;
    let commit_files = get_commit_files(&cwd, &args.commit)?;

    let short_sha = &commit_info.sha[..8.min(commit_info.sha.len())];

    println!("Auto-linking to commit {}", short_sha.yellow());
    println!(
        "  Commit: {} ({})",
        commit_info.summary.dimmed(),
        commit_info.timestamp.format("%Y-%m-%d %H:%M")
    );
    println!("  Files changed: {}", commit_files.len());
    println!("  Threshold: {:.0}%", threshold * 100.0);
    println!();

    // Get working directory for filtering sessions
    let repo_path = get_repo_root(&cwd)?;

    // Find sessions active near the commit time
    let candidates = db.find_sessions_near_commit_time(
        commit_info.timestamp,
        DEFAULT_WINDOW_MINUTES,
        Some(&repo_path),
    )?;

    if candidates.is_empty() {
        println!(
            "{}",
            "No sessions found within time window of commit.".yellow()
        );
        return Ok(());
    }

    println!("Found {} candidate session(s)", candidates.len());

    // Score and filter sessions
    let mut linked_count = 0;
    let mut skipped_existing = 0;

    for session in &candidates {
        // Check if already linked
        if db.link_exists(&session.id, &commit_info.sha)? {
            skipped_existing += 1;
            continue;
        }

        // Get session files
        let messages = db.get_messages(&session.id)?;
        let session_files = extract_session_files(&messages, &session.working_directory);

        // Calculate time difference in minutes
        let session_end = session.ended_at.unwrap_or_else(Utc::now);
        let time_diff = (commit_info.timestamp - session_end).num_minutes().abs();

        // Calculate confidence score
        let commit_branch = commit_info.branch.as_deref().unwrap_or("unknown");
        let confidence = calculate_link_confidence(
            session.git_branch.as_deref(),
            &session_files,
            commit_branch,
            &commit_files,
            time_diff,
        );

        let session_short_id = &session.id.to_string()[..8];

        if confidence >= threshold {
            if args.dry_run {
                println!(
                    "  {} Would link {} (confidence: {:.0}%)",
                    "[dry-run]".cyan(),
                    session_short_id.cyan(),
                    confidence * 100.0
                );
            } else {
                // Create the link
                let link = SessionLink {
                    id: Uuid::new_v4(),
                    session_id: session.id,
                    link_type: LinkType::Commit,
                    commit_sha: Some(commit_info.sha.clone()),
                    branch: commit_info.branch.clone(),
                    remote: None,
                    created_at: Utc::now(),
                    created_by: LinkCreator::Auto,
                    confidence: Some(confidence),
                };

                db.insert_link(&link)?;

                println!(
                    "  {} {} -> {} (confidence: {:.0}%)",
                    "Linked".green(),
                    session_short_id.cyan(),
                    short_sha,
                    confidence * 100.0
                );
            }
            linked_count += 1;
        } else {
            println!(
                "  {} {} (confidence: {:.0}% < {:.0}%)",
                "Skipped".dimmed(),
                session_short_id.dimmed(),
                confidence * 100.0,
                threshold * 100.0
            );
        }
    }

    println!();
    if args.dry_run {
        println!(
            "Dry run complete: would link {} session(s)",
            linked_count.to_string().green()
        );
    } else {
        println!("Linked {} session(s)", linked_count.to_string().green());
    }

    if skipped_existing > 0 {
        println!(
            "Skipped {} already-linked session(s)",
            skipped_existing.to_string().yellow()
        );
    }

    Ok(())
}

/// Resolves a commit reference to a full SHA.
fn resolve_commit(commit_ref: &str) -> Result<String> {
    let repo = git2::Repository::discover(".")
        .context("Not in a git repository. Use --commit to specify a commit SHA.")?;

    let obj = repo
        .revparse_single(commit_ref)
        .with_context(|| format!("Could not resolve commit: {commit_ref}"))?;

    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("{commit_ref} is not a commit"))?;

    Ok(commit.id().to_string())
}

/// Gets the root path of the git repository.
fn get_repo_root(path: &Path) -> Result<String> {
    let repo = git2::Repository::discover(path).context("Not a git repository")?;

    let workdir = repo
        .workdir()
        .context("Could not get repository working directory")?;

    Ok(workdir.to_string_lossy().to_string())
}

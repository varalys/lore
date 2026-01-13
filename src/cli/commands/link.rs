//! Link command - link sessions to git commits.
//!
//! Creates associations between development sessions and the commits
//! they produced. Links are stored in the database and can be queried
//! by commit SHA to find related sessions.

use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use std::path::Path;
use uuid::Uuid;

use crate::storage::{Database, LinkCreator, LinkType, SessionLink};

use crate::config::Config;
use crate::git::{
    calculate_link_confidence, get_commit_files, get_commit_info, get_commits_in_time_range,
};
use crate::storage::extract_session_files;

/// Default time window in minutes for finding sessions near a commit.
const DEFAULT_WINDOW_MINUTES: i64 = 30;

/// Arguments for the link command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore link abc123                    Link session to HEAD\n    \
    lore link abc123 def456             Link multiple sessions\n    \
    lore link abc123 --commit 1a2b3c    Link to specific commit\n    \
    lore link abc123 --dry-run          Preview without linking\n    \
    lore link --auto                    Preview auto-link suggestions\n    \
    lore link --auto --yes              Apply auto-link suggestions\n    \
    lore link --auto --backfill         Preview backfill suggestions\n    \
    lore link --auto --backfill --yes   Apply backfill suggestions\n    \
    lore link --current                 Link active sessions in this repo")]
pub struct Args {
    /// Session ID prefixes to link (can specify multiple)
    #[arg(value_name = "SESSION")]
    #[arg(
        long_help = "One or more session ID prefixes to link. You only need to\n\
        provide enough characters to uniquely identify each session."
    )]
    pub sessions: Vec<String>,

    /// Commit to link to (defaults to HEAD)
    #[arg(long, default_value = "HEAD", value_name = "REF")]
    #[arg(
        long_help = "The git commit to link sessions to. Accepts any git reference:\n\
        SHA, HEAD, HEAD~1, branch name, tag, etc. Defaults to HEAD."
    )]
    pub commit: String,

    /// Link currently active sessions in this repository
    #[arg(long)]
    #[arg(
        long_help = "Automatically finds and links sessions that are currently active\n\
        (or ended within the last 5 minutes) in this git repository. This is\n\
        used by the post-commit hook for forward auto-linking."
    )]
    pub current: bool,

    /// Auto-link sessions to a commit based on heuristics
    #[arg(long)]
    pub auto: bool,

    /// Backfill session-to-commit links using session time windows
    #[arg(long)]
    pub backfill: bool,

    /// Auto-link confidence threshold (0.0 - 1.0)
    #[arg(long)]
    pub threshold: Option<f64>,

    /// Apply auto-linking without prompting
    #[arg(long)]
    pub yes: bool,

    /// Preview what would be linked without making changes
    #[arg(long)]
    #[arg(
        long_help = "Shows what links would be created without actually modifying\n\
        the database. Useful for previewing results."
    )]
    pub dry_run: bool,
}

/// Executes the link command.
///
/// Creates links between the specified sessions and a commit.
/// Uses the current HEAD if no commit is specified.
pub fn run(args: Args) -> Result<()> {
    if args.current {
        run_current_link(args)
    } else if args.auto {
        if args.backfill {
            run_backfill_auto_link(args)
        } else {
            run_auto_link(args)
        }
    } else {
        run_manual_link(args)
    }
}

/// Runs manual linking for explicitly specified session IDs.
fn run_manual_link(args: Args) -> Result<()> {
    if args.sessions.is_empty() {
        anyhow::bail!(
            "No sessions specified. Provide one or more session IDs to link.\n\
             Run 'lore sessions' to list available sessions."
        );
    }

    let db = Database::open_default()?;

    // Resolve commit
    let commit_sha = resolve_commit(&args.commit)?;
    let short_sha = &commit_sha[..8.min(commit_sha.len())];
    println!("Linking to commit {}", short_sha.yellow());

    // Find and link each session using efficient database lookup
    for session_prefix in &args.sessions {
        let session = match db.find_session_by_id_prefix(session_prefix)? {
            Some(s) => s,
            None => {
                // Check if database is empty for a better error message
                if db.session_count()? == 0 {
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

/// Links currently active sessions in this repository to a commit.
///
/// This is the forward auto-linking implementation. It finds sessions that:
/// - Have a working_directory matching the current repository root
/// - Are currently active (no ended_at) OR ended within the last 5 minutes
///
/// Unlike retroactive auto-linking, no confidence scoring is needed because
/// if a session is active in this repo at commit time, it is the right session.
fn run_current_link(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Get repository root
    let cwd = std::env::current_dir()?;
    let repo_path = get_repo_root(&cwd)?;

    // Resolve commit
    let commit_sha = resolve_commit(&args.commit)?;
    let short_sha = &commit_sha[..8.min(commit_sha.len())];

    // Find active sessions for this directory
    let sessions = db.find_active_sessions_for_directory(&repo_path, None)?;

    if sessions.is_empty() {
        // Silent exit - this is expected when no AI session is active
        // The post-commit hook calls this on every commit
        return Ok(());
    }

    println!("Linking active sessions to commit {}", short_sha.yellow());

    let mut linked_count = 0;
    let mut skipped_existing = 0;

    for session in &sessions {
        // Check if already linked to avoid duplicates
        if db.link_exists(&session.id, &commit_sha)? {
            skipped_existing += 1;
            continue;
        }

        let session_short_id = &session.id.to_string()[..8];

        if args.dry_run {
            println!(
                "  {} Would link session {} -> commit {}",
                "[dry-run]".cyan(),
                session_short_id.cyan(),
                short_sha
            );
            linked_count += 1;
            continue;
        }

        let link = SessionLink {
            id: Uuid::new_v4(),
            session_id: session.id,
            link_type: LinkType::Commit,
            commit_sha: Some(commit_sha.clone()),
            branch: session.git_branch.clone(),
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::Auto,
            confidence: None, // Forward linking does not need confidence
        };

        db.insert_link(&link)?;

        println!(
            "  {} session {} -> commit {}",
            "Linked".green(),
            session_short_id.cyan(),
            short_sha
        );
        linked_count += 1;
    }

    if linked_count > 0 || skipped_existing > 0 {
        println!();
        if args.dry_run {
            println!(
                "Dry run complete: would link {} session(s)",
                linked_count.to_string().green()
            );
        } else if linked_count > 0 {
            println!("Linked {} session(s)", linked_count.to_string().green());
        }

        if skipped_existing > 0 {
            println!(
                "Skipped {} already-linked session(s)",
                skipped_existing.to_string().yellow()
            );
        }
    }

    Ok(())
}

/// Runs automatic linking based on heuristics.
///
/// Finds sessions active near a commit and scores them by time proximity,
/// file overlap, and branch matching. Shows a preview and requires --yes to apply.
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
    let mut skipped_existing = 0;
    let mut proposed: Vec<(String, Uuid, f64)> = Vec::new();

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
            proposed.push((session_short_id.to_string(), session.id, confidence));
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
    if proposed.is_empty() {
        println!("{}", "No sessions met the confidence threshold.".yellow());
    } else {
        println!(
            "{} session(s) meet the confidence threshold:",
            proposed.len().to_string().green()
        );
        for (session_short_id, _session_id, confidence) in &proposed {
            println!(
                "  {} Would link {} -> {} (confidence: {:.0}%)",
                "[dry-run]".cyan(),
                session_short_id.cyan(),
                short_sha,
                confidence * 100.0
            );
        }
    }

    if skipped_existing > 0 {
        println!(
            "Skipped {} already-linked session(s)",
            skipped_existing.to_string().yellow()
        );
    }

    if args.dry_run || proposed.is_empty() {
        return Ok(());
    }

    if !args.yes {
        let mut input = String::new();
        print!("Apply these links? (y/N): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        if trimmed != "y" && trimmed != "yes" {
            println!("{}", "Aborted; no links created.".yellow());
            return Ok(());
        }
    }

    let mut linked_count = 0;
    for (_session_short_id, session_id, confidence) in proposed {
        let link = SessionLink {
            id: Uuid::new_v4(),
            session_id,
            link_type: LinkType::Commit,
            commit_sha: Some(commit_info.sha.clone()),
            branch: commit_info.branch.clone(),
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::Auto,
            confidence: Some(confidence),
        };

        db.insert_link(&link)?;
        linked_count += 1;
    }

    println!("Linked {} session(s)", linked_count.to_string().green());

    Ok(())
}

/// Runs automatic backfill linking based on session time windows.
///
/// This scans ended sessions and links commits that fall between
/// started_at and ended_at for each session.
fn run_backfill_auto_link(args: Args) -> Result<()> {
    let db = Database::open_default()?;
    // Use a high limit to effectively scan all sessions
    let sessions = db.list_ended_sessions(1_000_000, None)?;
    let total_sessions = sessions.len();

    if sessions.is_empty() {
        println!("{}", "No ended sessions found.".yellow());
        return Ok(());
    }

    let mut proposed: Vec<(Uuid, String, String, String)> = Vec::new();
    let mut skipped_existing = 0usize;
    let mut skipped_missing_dir = 0usize;
    let mut skipped_non_git = 0usize;
    let mut sessions_with_existing_dir = 0usize;
    let mut sessions_in_git_repo = 0usize;

    for session in sessions {
        let ended_at = match session.ended_at {
            Some(ended_at) => ended_at,
            None => continue,
        };

        let working_dir = Path::new(&session.working_directory);
        if !working_dir.exists() {
            skipped_missing_dir += 1;
            continue;
        }
        sessions_with_existing_dir += 1;

        let commits = match get_commits_in_time_range(working_dir, session.started_at, ended_at) {
            Ok(commits) => commits,
            Err(_) => {
                skipped_non_git += 1;
                continue;
            }
        };
        sessions_in_git_repo += 1;

        if commits.is_empty() {
            continue;
        }

        let session_short_id = session.id.to_string();
        let session_short_id = &session_short_id[..8.min(session_short_id.len())];

        for commit in commits {
            if db.link_exists(&session.id, &commit.sha)? {
                skipped_existing += 1;
                continue;
            }

            let commit_short = &commit.sha[..8.min(commit.sha.len())];
            proposed.push((
                session.id,
                session_short_id.to_string(),
                commit.sha.clone(),
                format!(
                    "{} {}",
                    commit_short,
                    commit.summary.chars().take(60).collect::<String>()
                ),
            ));
        }
    }

    if proposed.is_empty() {
        println!("{}", "No backfill links found.".yellow());
    } else {
        println!(
            "{} session-to-commit link(s) found:",
            proposed.len().to_string().green()
        );
        for (_session_id, session_short_id, _commit_sha, commit_label) in &proposed {
            println!(
                "  {} Would link {} -> {}",
                "[dry-run]".cyan(),
                session_short_id.cyan(),
                commit_label
            );
        }
    }

    println!(
        "Scanned {} ended session(s); {} with existing directories; {} in git repos",
        total_sessions, sessions_with_existing_dir, sessions_in_git_repo
    );

    if skipped_existing > 0 {
        println!(
            "Skipped {} already-linked commit(s)",
            skipped_existing.to_string().yellow()
        );
    }
    if skipped_missing_dir > 0 {
        println!(
            "Skipped {} session(s) with missing directories",
            skipped_missing_dir.to_string().yellow()
        );
    }
    if skipped_non_git > 0 {
        println!(
            "Skipped {} session(s) in non-git directories",
            skipped_non_git.to_string().yellow()
        );
    }

    if args.dry_run || proposed.is_empty() {
        return Ok(());
    }

    if !args.yes {
        let mut input = String::new();
        print!("Apply these links? (y/N): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        if trimmed != "y" && trimmed != "yes" {
            println!("{}", "Aborted; no links created.".yellow());
            return Ok(());
        }
    }

    let mut linked_count = 0usize;
    for (session_id, _session_short_id, commit_sha, _commit_label) in proposed {
        let link = SessionLink {
            id: Uuid::new_v4(),
            session_id,
            link_type: LinkType::Commit,
            commit_sha: Some(commit_sha),
            branch: None,
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::Auto,
            confidence: Some(1.0),
        };

        db.insert_link(&link)?;
        linked_count += 1;
    }

    println!("Linked {} session(s)", linked_count.to_string().green());

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

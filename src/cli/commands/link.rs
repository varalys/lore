//! Link command - link sessions to commits

use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use uuid::Uuid;

use crate::storage::{Database, LinkCreator, LinkType, SessionLink};

#[derive(clap::Args)]
pub struct Args {
    /// Session IDs to link (can specify multiple)
    #[arg(required = true)]
    pub sessions: Vec<String>,

    /// Commit SHA to link to (defaults to HEAD)
    #[arg(long)]
    pub commit: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Resolve commit
    let commit_sha = if let Some(ref sha) = args.commit {
        sha.clone()
    } else {
        // Try to get HEAD from git
        get_head_commit()?
    };

    let short_sha = &commit_sha[..8.min(commit_sha.len())];
    println!("Linking to commit {}", short_sha.yellow());

    // Find and link each session
    let all_sessions = db.list_sessions(1000, None)?;

    for session_prefix in &args.sessions {
        let session = all_sessions
            .iter()
            .find(|s| s.id.to_string().starts_with(session_prefix))
            .context(format!("No session found matching '{}'", session_prefix))?;

        let link = SessionLink {
            id: Uuid::new_v4(),
            session_id: session.id,
            link_type: LinkType::Commit,
            commit_sha: Some(commit_sha.clone()),
            branch: None, // Could get from git
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::User,
            confidence: None,
        };

        db.insert_link(&link)?;

        println!(
            "  {} session {} → commit {}",
            "Linked".green(),
            &session.id.to_string()[..8].cyan(),
            short_sha
        );
    }

    Ok(())
}

fn get_head_commit() -> Result<String> {
    // Try to use git2 to get HEAD
    let repo = git2::Repository::discover(".").context(
        "Not in a git repository. Use --commit to specify a commit SHA.",
    )?;

    let head = repo.head().context("Could not get HEAD")?;
    let commit = head.peel_to_commit().context("HEAD is not a commit")?;

    Ok(commit.id().to_string())
}

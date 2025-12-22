//! Git integration
//!
//! TODO: Implement auto-linking, hooks, etc.

use anyhow::{Context, Result};
use std::path::Path;

/// Get information about the current git repository
pub fn repo_info(path: &Path) -> Result<RepoInfo> {
    let repo = git2::Repository::discover(path).context("Not a git repository")?;

    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .and_then(|h| h.shorthand())
        .map(|s| s.to_string());

    let commit_sha = head
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id().to_string());

    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(|s| s.to_string()));

    let workdir = repo
        .workdir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    Ok(RepoInfo {
        path: workdir,
        branch,
        commit_sha,
        remote_url,
    })
}

#[derive(Debug)]
pub struct RepoInfo {
    pub path: String,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub remote_url: Option<String>,
}

/// Calculate relevance score for auto-linking a session to a commit
pub fn calculate_link_confidence(
    session_branch: Option<&str>,
    session_files: &[String],
    commit_branch: &str,
    commit_files: &[String],
    time_diff_minutes: i64,
) -> f64 {
    let mut score = 0.0;

    // Branch match
    if session_branch == Some(commit_branch) {
        score += 0.2;
    }

    // File overlap
    let overlap = session_files
        .iter()
        .filter(|f| commit_files.contains(f))
        .count();

    if overlap > 0 {
        let overlap_ratio = overlap as f64 / commit_files.len().max(1) as f64;
        score += 0.4 * overlap_ratio;
    }

    // Time proximity (decay over 30 minutes)
    if time_diff_minutes < 30 {
        score += 0.3 * (1.0 - (time_diff_minutes as f64 / 30.0));
    }

    // Recent activity bonus
    if time_diff_minutes < 5 {
        score += 0.1;
    }

    score.min(1.0)
}

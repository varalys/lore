//! Git integration.
//!
//! Provides git repository discovery, commit information retrieval,
//! and auto-linking confidence scoring. Used by the link command and
//! future auto-linking features.
//!
//! Note: Auto-linking and git hooks are not yet fully implemented.

use anyhow::{Context, Result};
use std::path::Path;

/// Retrieves information about a git repository.
///
/// Discovers the repository containing the given path and extracts
/// branch, commit, and remote information.
///
/// # Errors
///
/// Returns an error if the path is not inside a git repository.
#[allow(dead_code)]
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

/// Information about a git repository.
///
/// Contains the current state of a repository including branch,
/// HEAD commit, and remote URL.
#[derive(Debug)]
#[allow(dead_code)]
pub struct RepoInfo {
    /// Absolute path to the repository working directory.
    pub path: String,
    /// Current branch name, if HEAD points to a branch.
    pub branch: Option<String>,
    /// SHA of the current HEAD commit.
    pub commit_sha: Option<String>,
    /// URL of the "origin" remote, if configured.
    pub remote_url: Option<String>,
}

/// Calculates a confidence score for auto-linking a session to a commit.
///
/// The score is based on multiple factors:
/// - Branch match (20%): Session and commit are on the same branch
/// - File overlap (40%): Proportion of commit files mentioned in the session
/// - Time proximity (30%): Decays over 30 minutes
/// - Recent activity bonus (10%): Extra weight for commits within 5 minutes
///
/// Returns a value between 0.0 and 1.0.
#[allow(dead_code)]
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

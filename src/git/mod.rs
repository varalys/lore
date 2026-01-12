//! Git integration.
//!
//! Provides git repository discovery, commit information retrieval,
//! and auto-linking confidence scoring. Used by the link command and
//! auto-linking features.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashSet;
use std::path::Path;

/// Retrieves information about a git repository.
///
/// Discovers the repository containing the given path and extracts
/// branch, commit, and remote information.
///
/// # Errors
///
/// Returns an error if the path is not inside a git repository.
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
pub struct RepoInfo {
    /// Absolute path to the repository working directory.
    /// Currently used for session filtering by working directory.
    #[allow(dead_code)]
    pub path: String,
    /// Current branch name, if HEAD points to a branch.
    /// Used for branch-based session matching in auto-linking.
    #[allow(dead_code)]
    pub branch: Option<String>,
    /// SHA of the current HEAD commit.
    pub commit_sha: Option<String>,
    /// URL of the "origin" remote, if configured.
    /// Reserved for future remote-based features.
    #[allow(dead_code)]
    pub remote_url: Option<String>,
}

/// Information about a specific git commit.
///
/// Contains the SHA, timestamp, branch, and author information.
#[derive(Debug)]
pub struct CommitInfo {
    /// Full SHA of the commit.
    pub sha: String,
    /// When the commit was authored.
    pub timestamp: DateTime<Utc>,
    /// Branch name the commit is on (if determinable).
    pub branch: Option<String>,
    /// Commit message summary (first line).
    pub summary: String,
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

/// Retrieves all commits in a repository made within a time range.
///
/// Walks the commit history from all local branches and collects all commits
/// whose timestamps fall between `after` and `before` (inclusive on both ends).
/// Commits are returned in reverse chronological order (newest first).
///
/// This is equivalent to `git log --all --after=X --before=Y`.
///
/// This is used for auto-linking sessions to commits made during the
/// session's time window.
///
/// # Arguments
///
/// * `repo_path` - A path inside the git repository
/// * `after` - Only include commits at or after this time
/// * `before` - Only include commits at or before this time
///
/// # Errors
///
/// Returns an error if the repository cannot be found or the commit
/// history cannot be walked.
pub fn get_commits_in_time_range(
    repo_path: &Path,
    after: DateTime<Utc>,
    before: DateTime<Utc>,
) -> Result<Vec<CommitInfo>> {
    let repo = git2::Repository::discover(repo_path).context("Not a git repository")?;

    let mut revwalk = repo.revwalk().context("Could not create revision walker")?;

    // Push all local branch refs to walk commits from all branches
    for branch_result in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch_result?;
        if let Some(reference) = branch.get().target() {
            revwalk.push(reference)?;
        }
    }

    revwalk.set_sorting(git2::Sort::TIME)?;

    let after_secs = after.timestamp();
    let before_secs = before.timestamp();

    let mut commits = Vec::new();
    let mut seen_shas: HashSet<String> = HashSet::new();

    for oid_result in revwalk {
        let oid = oid_result.context("Error walking commits")?;
        let commit = repo.find_commit(oid).context("Could not find commit")?;

        let sha = commit.id().to_string();

        // Skip already-seen commits (same commit reachable from multiple branches)
        if seen_shas.contains(&sha) {
            continue;
        }
        seen_shas.insert(sha.clone());

        let commit_time = commit.time().seconds();

        // Skip commits outside the time window
        // Note: we cannot do early exit because commits from different branches
        // may interleave in the time-sorted order
        if commit_time < after_secs || commit_time > before_secs {
            continue;
        }

        let timestamp = Utc
            .timestamp_opt(commit_time, 0)
            .single()
            .unwrap_or_else(Utc::now);

        // Try to determine branch name by checking if HEAD points to this commit
        let branch = repo.head().ok().and_then(|h| {
            if h.peel_to_commit().ok()?.id() == commit.id() {
                h.shorthand().map(|s| s.to_string())
            } else {
                None
            }
        });

        let summary = commit.summary().unwrap_or("").to_string();

        commits.push(CommitInfo {
            sha,
            timestamp,
            branch,
            summary,
        });
    }

    Ok(commits)
}

/// Retrieves information about a specific commit.
///
/// Resolves the commit reference (SHA, HEAD, branch name, etc.) and returns
/// details including timestamp, branch, and summary.
///
/// # Errors
///
/// Returns an error if the repository cannot be found or the commit
/// reference cannot be resolved.
pub fn get_commit_info(repo_path: &Path, commit_ref: &str) -> Result<CommitInfo> {
    let repo = git2::Repository::discover(repo_path).context("Not a git repository")?;

    // Resolve the reference to a commit
    let obj = repo
        .revparse_single(commit_ref)
        .with_context(|| format!("Could not resolve commit reference: {commit_ref}"))?;

    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("Reference is not a commit: {commit_ref}"))?;

    let sha = commit.id().to_string();

    // Convert git timestamp to chrono DateTime
    let git_time = commit.time();
    let timestamp = Utc
        .timestamp_opt(git_time.seconds(), 0)
        .single()
        .unwrap_or_else(Utc::now);

    // Try to get the branch name (check if HEAD points to this commit)
    let branch = repo.head().ok().and_then(|h| {
        if h.peel_to_commit().ok()?.id() == commit.id() {
            h.shorthand().map(|s| s.to_string())
        } else {
            None
        }
    });

    let summary = commit.summary().unwrap_or("").to_string();

    Ok(CommitInfo {
        sha,
        timestamp,
        branch,
        summary,
    })
}

/// Resolves a git reference (SHA, HEAD, branch name, etc.) to a full commit SHA.
///
/// Supports:
/// - Full and partial SHAs
/// - HEAD and HEAD~N syntax
/// - Branch names
/// - Tag names
///
/// # Arguments
///
/// * `repo_path` - A path inside the git repository
/// * `reference` - The git reference to resolve (SHA, HEAD, branch, tag, etc.)
///
/// # Errors
///
/// Returns an error if the repository cannot be found or the reference
/// cannot be resolved to a valid commit.
pub fn resolve_commit_ref(repo_path: &Path, reference: &str) -> Result<String> {
    let repo = git2::Repository::discover(repo_path).context("Not a git repository")?;

    // Resolve the reference to a commit
    let obj = repo
        .revparse_single(reference)
        .with_context(|| format!("Could not resolve reference: {reference}"))?;

    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("Reference is not a commit: {reference}"))?;

    Ok(commit.id().to_string())
}

/// Retrieves the list of files changed in a commit.
///
/// Returns the file paths relative to the repository root for all files
/// that were added, modified, or deleted in the commit.
///
/// # Errors
///
/// Returns an error if the repository cannot be found or the commit
/// reference cannot be resolved.
pub fn get_commit_files(repo_path: &Path, commit_ref: &str) -> Result<Vec<String>> {
    let repo = git2::Repository::discover(repo_path).context("Not a git repository")?;

    // Resolve the reference to a commit
    let obj = repo
        .revparse_single(commit_ref)
        .with_context(|| format!("Could not resolve commit reference: {commit_ref}"))?;

    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("Reference is not a commit: {commit_ref}"))?;

    let tree = commit.tree().context("Could not get commit tree")?;

    // Get the parent tree (or empty tree for initial commit)
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .context("Could not compute diff")?;

    let mut files = Vec::new();

    diff.foreach(
        &mut |delta, _| {
            // Get the new file path (or old path for deletions)
            let path = delta.new_file().path().or_else(|| delta.old_file().path());

            if let Some(p) = path {
                files.push(p.to_string_lossy().to_string());
            }
            true
        },
        None,
        None,
        None,
    )
    .context("Could not iterate diff")?;

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_link_confidence_full_match() {
        let session_files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let commit_files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];

        let score = calculate_link_confidence(
            Some("main"),
            &session_files,
            "main",
            &commit_files,
            2, // 2 minutes ago
        );

        // Branch match: 0.2
        // File overlap: 0.4 (100% overlap)
        // Time proximity: 0.3 * (1 - 2/30) = 0.28
        // Recent bonus: 0.1 (within 5 min)
        // Total: 0.98
        assert!(
            score > 0.9,
            "Full match should have high confidence: {score}"
        );
    }

    #[test]
    fn test_calculate_link_confidence_no_match() {
        let session_files = vec!["other.rs".to_string()];
        let commit_files = vec!["src/main.rs".to_string()];

        let score = calculate_link_confidence(
            Some("feature"),
            &session_files,
            "main",
            &commit_files,
            60, // 60 minutes ago
        );

        // Branch match: 0 (different)
        // File overlap: 0 (no overlap)
        // Time proximity: 0 (> 30 min)
        // Recent bonus: 0 (> 5 min)
        // Total: 0
        assert!(score < 0.1, "No match should have low confidence: {score}");
    }

    #[test]
    fn test_calculate_link_confidence_partial_overlap() {
        let session_files = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "other.rs".to_string(),
        ];
        let commit_files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];

        let score = calculate_link_confidence(
            Some("main"),
            &session_files,
            "main",
            &commit_files,
            15, // 15 minutes ago
        );

        // Branch match: 0.2
        // File overlap: 0.4 (100% of commit files are in session files)
        // Time proximity: 0.3 * (1 - 15/30) = 0.15
        // Recent bonus: 0 (> 5 min)
        // Total: 0.75
        assert!(
            score > 0.7 && score < 0.8,
            "Partial match should have medium-high confidence: {score}"
        );
    }

    #[test]
    fn test_calculate_link_confidence_time_decay() {
        let session_files = vec!["src/main.rs".to_string()];
        let commit_files = vec!["src/main.rs".to_string()];

        let score_recent =
            calculate_link_confidence(Some("main"), &session_files, "main", &commit_files, 1);

        let score_old =
            calculate_link_confidence(Some("main"), &session_files, "main", &commit_files, 25);

        assert!(
            score_recent > score_old,
            "Recent commits should score higher: {score_recent} vs {score_old}"
        );
    }

    #[test]
    fn test_calculate_link_confidence_caps_at_one() {
        let session_files = vec!["a.rs".to_string(), "b.rs".to_string()];
        let commit_files = vec!["a.rs".to_string()];

        let score =
            calculate_link_confidence(Some("main"), &session_files, "main", &commit_files, 0);

        assert!(score <= 1.0, "Score should be capped at 1.0: {score}");
    }

    #[test]
    fn test_calculate_link_confidence_empty_files() {
        let session_files: Vec<String> = vec![];
        let commit_files: Vec<String> = vec![];

        let score =
            calculate_link_confidence(Some("main"), &session_files, "main", &commit_files, 5);

        // Should not panic and should give branch + time score
        assert!(score > 0.0, "Should handle empty files gracefully: {score}");
    }

    // ==================== resolve_commit_ref Tests ====================

    #[test]
    fn test_resolve_commit_ref_with_head() {
        // This test runs in the lore repository itself
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // HEAD should always resolve to a valid SHA
        let result = resolve_commit_ref(repo_path, "HEAD");
        assert!(result.is_ok(), "HEAD should resolve: {:?}", result.err());

        let sha = result.unwrap();
        // SHA should be 40 hex characters
        assert_eq!(sha.len(), 40, "SHA should be 40 characters: {sha}");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "SHA should be hex: {sha}"
        );
    }

    #[test]
    fn test_resolve_commit_ref_with_head_tilde() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // HEAD~1 should resolve if there are at least 2 commits
        // This may fail in a fresh repo with only one commit
        let result = resolve_commit_ref(repo_path, "HEAD~1");

        // If the repo has multiple commits, this should succeed
        if let Ok(sha) = result {
            assert_eq!(sha.len(), 40, "SHA should be 40 characters");

            // Should be different from HEAD
            let head_sha = resolve_commit_ref(repo_path, "HEAD").unwrap();
            assert_ne!(sha, head_sha, "HEAD~1 should differ from HEAD");
        }
        // If it fails, that's acceptable for a repo with one commit
    }

    #[test]
    fn test_resolve_commit_ref_with_full_sha() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // First get HEAD's SHA
        let head_sha = resolve_commit_ref(repo_path, "HEAD").unwrap();

        // Now resolve using the full SHA
        let result = resolve_commit_ref(repo_path, &head_sha);
        assert!(
            result.is_ok(),
            "Full SHA should resolve: {:?}",
            result.err()
        );

        let resolved = result.unwrap();
        assert_eq!(resolved, head_sha, "Resolved SHA should match input");
    }

    #[test]
    fn test_resolve_commit_ref_with_partial_sha() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // First get HEAD's SHA
        let head_sha = resolve_commit_ref(repo_path, "HEAD").unwrap();

        // Try resolving with first 7 characters (common short SHA length)
        let short_sha = &head_sha[..7];
        let result = resolve_commit_ref(repo_path, short_sha);
        assert!(
            result.is_ok(),
            "Partial SHA should resolve: {:?}",
            result.err()
        );

        let resolved = result.unwrap();
        assert_eq!(resolved, head_sha, "Resolved SHA should be full SHA");
    }

    #[test]
    fn test_resolve_commit_ref_invalid_reference() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // This reference should not exist
        let result = resolve_commit_ref(repo_path, "nonexistent-branch-xyz123");
        assert!(result.is_err(), "Invalid reference should fail");
    }

    #[test]
    fn test_resolve_commit_ref_not_a_repo() {
        // /tmp should not be a git repository
        let result = resolve_commit_ref(std::path::Path::new("/tmp"), "HEAD");
        assert!(result.is_err(), "Non-repo path should fail");
    }

    // ==================== get_commits_in_time_range Tests ====================

    #[test]
    fn test_get_commits_in_time_range_returns_recent_commits() {
        // This test runs in the lore repository itself
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // Get HEAD commit info to know when it was made
        let head_info = get_commit_info(repo_path, "HEAD").expect("Should get HEAD commit info");

        // Create a time range that includes HEAD commit (from 1 hour before to 1 hour after)
        let after = head_info.timestamp - chrono::Duration::hours(1);
        let before = head_info.timestamp + chrono::Duration::hours(1);

        let result = get_commits_in_time_range(repo_path, after, before);
        assert!(
            result.is_ok(),
            "Should get commits in time range: {:?}",
            result.err()
        );

        let commits = result.unwrap();
        assert!(
            !commits.is_empty(),
            "Should find at least HEAD commit in time range"
        );

        // HEAD commit should be in the results
        let has_head = commits.iter().any(|c| c.sha == head_info.sha);
        assert!(has_head, "HEAD commit should be in results");
    }

    #[test]
    fn test_get_commits_in_time_range_empty_for_future() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // Create a time range entirely in the future
        let now = Utc::now();
        let after = now + chrono::Duration::days(365);
        let before = now + chrono::Duration::days(366);

        let result = get_commits_in_time_range(repo_path, after, before);
        assert!(result.is_ok(), "Should succeed even with future dates");

        let commits = result.unwrap();
        assert!(
            commits.is_empty(),
            "Should find no commits in future time range"
        );
    }

    #[test]
    fn test_get_commits_in_time_range_empty_for_distant_past() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // Create a time range before git was invented
        let after = Utc.with_ymd_and_hms(1990, 1, 1, 0, 0, 0).unwrap();
        let before = Utc.with_ymd_and_hms(1990, 1, 2, 0, 0, 0).unwrap();

        let result = get_commits_in_time_range(repo_path, after, before);
        assert!(result.is_ok(), "Should succeed even with past dates");

        let commits = result.unwrap();
        assert!(
            commits.is_empty(),
            "Should find no commits in distant past time range"
        );
    }

    #[test]
    fn test_get_commits_in_time_range_not_a_repo() {
        // /tmp should not be a git repository
        let after = Utc::now() - chrono::Duration::hours(1);
        let before = Utc::now();

        let result = get_commits_in_time_range(std::path::Path::new("/tmp"), after, before);
        assert!(result.is_err(), "Non-repo path should fail");
    }

    #[test]
    fn test_get_commits_in_time_range_commit_info_complete() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // Get HEAD commit to determine time range
        let head_info = get_commit_info(repo_path, "HEAD").expect("Should get HEAD commit info");

        let after = head_info.timestamp - chrono::Duration::seconds(1);
        let before = head_info.timestamp + chrono::Duration::seconds(1);

        let commits =
            get_commits_in_time_range(repo_path, after, before).expect("Should get commits");

        // Find HEAD commit in results
        let head_commit = commits.iter().find(|c| c.sha == head_info.sha);
        assert!(head_commit.is_some(), "HEAD commit should be in results");

        let head_commit = head_commit.unwrap();

        // Verify commit info is complete
        assert_eq!(head_commit.sha.len(), 40, "SHA should be 40 characters");
        assert!(
            head_commit.sha.chars().all(|c| c.is_ascii_hexdigit()),
            "SHA should be hex"
        );
        assert_eq!(
            head_commit.timestamp, head_info.timestamp,
            "Timestamp should match"
        );
        // Summary should be non-empty for most commits
        // (we don't strictly require this as some commits may have empty messages)
    }

    #[test]
    fn test_get_commits_in_time_range_sorted_by_time() {
        let repo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // Wide time range to get multiple commits
        let before = Utc::now();
        let after = before - chrono::Duration::days(30);

        let result = get_commits_in_time_range(repo_path, after, before);
        if result.is_err() {
            // Skip test if repo access fails
            return;
        }

        let commits = result.unwrap();
        if commits.len() < 2 {
            // Not enough commits to test sorting
            return;
        }

        // Verify commits are sorted newest first (descending by time)
        for window in commits.windows(2) {
            assert!(
                window[0].timestamp >= window[1].timestamp,
                "Commits should be sorted newest first: {} >= {}",
                window[0].timestamp,
                window[1].timestamp
            );
        }
    }
}

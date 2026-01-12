//! File system watcher for AI tool session files.
//!
//! Watches directories configured by the WatcherRegistry for new and modified
//! session files. Performs incremental parsing to efficiently handle file
//! updates without re-reading entire files.
//!
//! Currently supports:
//! - Claude Code JSONL files in `~/.claude/projects/`

use anyhow::{Context, Result};
use chrono::Utc;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::capture::watchers::{default_registry, Watcher};
use crate::git::get_commits_in_time_range;
use crate::storage::models::{LinkCreator, LinkType, SessionLink};
use crate::storage::Database;

use super::state::DaemonStats;

/// Database path for creating connections within the watcher.
/// rusqlite connections are not thread-safe, so we create a new
/// connection when needed rather than sharing one across threads.
#[derive(Clone)]
pub struct DbConfig {
    path: PathBuf,
}

impl DbConfig {
    /// Creates a new DbConfig for the default database location.
    pub fn default_config() -> Result<Self> {
        let path = crate::storage::db::default_db_path()?;
        Ok(Self { path })
    }

    /// Opens a new database connection.
    pub fn open(&self) -> Result<Database> {
        Database::open(&self.path)
    }
}

/// Watches for session file changes and imports new messages.
///
/// Tracks the byte position in each file to enable incremental reading,
/// avoiding the need to re-parse entire files on each modification.
pub struct SessionWatcher {
    /// Maps file paths to their last read position (byte offset).
    file_positions: HashMap<PathBuf, u64>,
    /// Directories to watch for session files.
    watch_dirs: Vec<PathBuf>,
    /// Database configuration for creating connections.
    db_config: DbConfig,
}

impl SessionWatcher {
    /// Creates a new SessionWatcher.
    ///
    /// Uses the default watcher registry to determine which directories
    /// to watch for session files.
    ///
    /// # Errors
    ///
    /// Returns an error if the database configuration cannot be created.
    pub fn new() -> Result<Self> {
        let registry = default_registry();
        let watch_dirs = registry.all_watch_paths();

        let db_config = DbConfig::default_config()?;

        Ok(Self {
            file_positions: HashMap::new(),
            watch_dirs,
            db_config,
        })
    }

    /// Returns the directories being watched.
    ///
    /// This method is part of the public API for status reporting
    /// and may be used by CLI commands in the future.
    #[allow(dead_code)]
    pub fn watch_dirs(&self) -> &[PathBuf] {
        &self.watch_dirs
    }

    /// Starts watching for file changes and processing them.
    ///
    /// This function runs until the shutdown signal is received.
    ///
    /// # Arguments
    ///
    /// * `stats` - Shared statistics to update on imports
    /// * `shutdown_rx` - Receiver that signals when to stop watching
    ///
    /// # Errors
    ///
    /// Returns an error if the watcher cannot be created or started.
    pub async fn watch(
        &mut self,
        stats: Arc<RwLock<DaemonStats>>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<()> {
        // Log which directories we will watch
        for dir in &self.watch_dirs {
            if dir.exists() {
                tracing::info!("Will watch for session files in {:?}", dir);
            } else {
                tracing::info!("Watch directory does not exist yet: {:?}", dir);
            }
        }

        // Create a channel for file events
        let (tx, mut rx) = mpsc::channel::<Vec<DebouncedEvent>>(100);

        // Create the debounced watcher
        let mut debouncer = new_debouncer(
            Duration::from_millis(500),
            move |events: Result<Vec<DebouncedEvent>, notify::Error>| {
                if let Ok(events) = events {
                    // Filter for JSONL and SQLite database files
                    let filtered: Vec<DebouncedEvent> = events
                        .into_iter()
                        .filter(|e| {
                            let ext = e.path.extension().and_then(|ext| ext.to_str());
                            matches!(ext, Some("jsonl") | Some("vscdb"))
                        })
                        .collect();

                    if !filtered.is_empty() {
                        let _ = tx.blocking_send(filtered);
                    }
                }
            },
        )
        .context("Failed to create file watcher")?;

        // Start watching directories that exist
        for dir in &self.watch_dirs {
            if dir.exists() {
                debouncer
                    .watcher()
                    .watch(dir, RecursiveMode::Recursive)
                    .context(format!("Failed to start watching directory {dir:?}"))?;

                tracing::info!("Watching for session files in {:?}", dir);
            }
        }

        // Do an initial scan (sync, before entering async loop)
        self.initial_scan(&stats).await?;

        // Process events
        loop {
            tokio::select! {
                Some(events) = rx.recv() => {
                    for event in events {
                        if let Err(e) = self.handle_file_event(&event.path, &stats).await {
                            let error_msg = e.to_string();
                            // Database unavailable errors are transient (e.g., during lore init)
                            // Log at debug level to avoid spam
                            if error_msg.contains("unable to open database")
                                || error_msg.contains("database is locked")
                            {
                                tracing::debug!(
                                    "Database temporarily unavailable for {:?}: {}",
                                    event.path,
                                    e
                                );
                            } else {
                                tracing::warn!(
                                    "Error handling file event for {:?}: {}",
                                    event.path,
                                    e
                                );
                                let mut stats_guard = stats.write().await;
                                stats_guard.errors += 1;
                            }
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    tracing::info!("Session watcher shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Opens a database connection for this operation.
    fn open_db(&self) -> Result<Database> {
        self.db_config.open()
    }

    /// Finds the watcher that owns a given file path.
    ///
    /// A watcher "owns" a path if one of its `watch_paths()` is an ancestor
    /// of the given path. This ensures files are parsed by the correct watcher
    /// rather than trying all watchers in arbitrary order.
    ///
    /// Returns `None` if no watcher claims the path.
    fn find_owning_watcher<'a>(
        path: &Path,
        watchers: &'a [&'a dyn Watcher],
    ) -> Option<&'a dyn Watcher> {
        for watcher in watchers {
            for watch_path in watcher.watch_paths() {
                if path.starts_with(&watch_path) {
                    return Some(*watcher);
                }
            }
        }
        None
    }

    /// Performs an initial scan of existing session files.
    ///
    /// Called when the watcher starts to import any sessions that were
    /// created while the daemon was not running. Uses the watcher registry
    /// to find session sources from all available watchers.
    async fn initial_scan(&mut self, stats: &Arc<RwLock<DaemonStats>>) -> Result<()> {
        tracing::info!("Performing initial scan of session files...");

        let registry = default_registry();
        let mut total_files = 0;

        for watcher in registry.available_watchers() {
            let watcher_name = watcher.info().name;
            match watcher.find_sources() {
                Ok(sources) => {
                    tracing::info!("Found {} sources for {}", sources.len(), watcher_name);
                    total_files += sources.len();

                    for path in sources {
                        // Process each file synchronously to avoid Send issues
                        match self.process_file_sync(&path) {
                            Ok(Some((sessions_imported, messages_imported))) => {
                                let mut stats_guard = stats.write().await;
                                stats_guard.sessions_imported += sessions_imported;
                                stats_guard.messages_imported += messages_imported;
                                stats_guard.files_watched = self.file_positions.len();
                            }
                            Ok(None) => {
                                // File was already imported, just track position
                            }
                            Err(e) => {
                                tracing::warn!("Failed to import {:?}: {}", path, e);
                                let mut stats_guard = stats.write().await;
                                stats_guard.errors += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to find sources for {}: {}", watcher_name, e);
                }
            }
        }

        {
            let mut stats_guard = stats.write().await;
            stats_guard.files_watched = total_files;
        }

        Ok(())
    }

    /// Handles a file system event for a session file.
    async fn handle_file_event(
        &mut self,
        path: &Path,
        stats: &Arc<RwLock<DaemonStats>>,
    ) -> Result<()> {
        let ext = path.extension().and_then(|e| e.to_str());

        // Skip files that are not session sources
        if !matches!(ext, Some("jsonl") | Some("vscdb")) {
            return Ok(());
        }

        // Skip agent files (Claude Code specific)
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("agent-") {
                return Ok(());
            }
        }

        // Check if file exists (might be a delete event)
        if !path.exists() {
            // File was deleted, remove from tracking
            self.file_positions.remove(path);
            return Ok(());
        }

        // Process the file synchronously
        match self.process_file_sync(path) {
            Ok(Some((sessions_imported, messages_imported))) => {
                let mut stats_guard = stats.write().await;
                stats_guard.sessions_imported += sessions_imported;
                stats_guard.messages_imported += messages_imported;
                stats_guard.files_watched = self.file_positions.len();
            }
            Ok(None) => {
                // File unchanged or already processed
            }
            Err(e) => {
                return Err(e);
            }
        }

        Ok(())
    }

    /// Processes a file synchronously, returning import counts if anything was imported.
    ///
    /// Returns `Ok(Some((sessions, messages)))` if data was imported,
    /// `Ok(None)` if the file was already processed, or an error.
    ///
    /// When a session file already exists in the database but has grown (new messages
    /// added), this function re-imports the entire file. The database layer handles
    /// deduplication: `insert_session` uses ON CONFLICT to update metadata, and
    /// `insert_message` uses ON CONFLICT DO NOTHING to skip duplicates.
    /// After re-import, auto-linking is triggered if the session has ended.
    fn process_file_sync(&mut self, path: &Path) -> Result<Option<(u64, u64)>> {
        let db = self.open_db()?;
        let path_str = path.to_string_lossy();
        let last_pos = self.file_positions.get(path).copied().unwrap_or(0);

        // Get current file size
        let metadata = std::fs::metadata(path).context("Failed to get file metadata")?;
        let current_size = metadata.len();

        if current_size <= last_pos {
            // File hasn't grown (might have been truncated)
            if current_size < last_pos {
                // File was truncated, reset position
                self.file_positions.insert(path.to_path_buf(), 0);
            }
            return Ok(None);
        }

        // Check if session already exists in the database
        let existing_session = db.get_session_by_source(&path_str)?;

        if let Some(existing) = existing_session {
            // Session exists but file has grown - re-import to get new messages
            // and updated metadata, then run auto-linking
            tracing::debug!(
                "Session {} exists but file has grown, re-importing for updates",
                &existing.id.to_string()[..8]
            );
            let result = self.update_existing_session(path, &db, &existing)?;
            self.file_positions.insert(path.to_path_buf(), current_size);
            return Ok(Some(result));
        }

        // New session file - import it
        let result = self.import_file_sync(path, &db)?;

        // Update tracked position
        self.file_positions.insert(path.to_path_buf(), current_size);

        Ok(Some(result))
    }

    /// Updates an existing session by re-importing the file.
    ///
    /// Re-parses the session file and updates the database. The database layer
    /// handles deduplication automatically. After updating, triggers auto-linking
    /// if the session has an ended_at timestamp.
    ///
    /// Uses path-based dispatch to find the correct watcher for the file.
    ///
    /// Returns (0, new_messages_imported) since the session already exists.
    fn update_existing_session(
        &self,
        path: &Path,
        db: &Database,
        existing_session: &crate::storage::models::Session,
    ) -> Result<(u64, u64)> {
        tracing::debug!("Updating existing session from: {:?}", path);

        let path_buf = path.to_path_buf();
        let registry = default_registry();
        let available = registry.available_watchers();

        // Find the watcher that owns this path
        let owning_watcher = match Self::find_owning_watcher(path, &available) {
            Some(w) => w,
            None => {
                tracing::debug!("No watcher owns path {:?}", path);
                return Ok((0, 0));
            }
        };

        // Parse with the owning watcher
        let parsed_sessions = match owning_watcher.parse_source(&path_buf) {
            Ok(sessions) => sessions,
            Err(e) => {
                tracing::debug!(
                    "Watcher {} could not parse {:?}: {}",
                    owning_watcher.info().name,
                    path,
                    e
                );
                return Ok((0, 0));
            }
        };

        if parsed_sessions.is_empty() {
            tracing::debug!(
                "Watcher {} returned no sessions for {:?}",
                owning_watcher.info().name,
                path
            );
            return Ok((0, 0));
        }

        let mut total_messages = 0u64;
        let mut updated_session: Option<crate::storage::models::Session> = None;

        for (session, messages) in parsed_sessions {
            if messages.is_empty() {
                continue;
            }

            // Update session metadata (ended_at, message_count, git_branch)
            // insert_session uses ON CONFLICT to update these fields
            db.insert_session(&session)?;

            // Track the most recent branch from messages
            let mut latest_branch: Option<String> = None;
            let mut new_message_count = 0u64;

            for msg in &messages {
                // insert_message uses ON CONFLICT DO NOTHING, so duplicates are skipped
                // We don't have a reliable way to count only new messages, but we track
                // the total for logging purposes
                db.insert_message(msg)?;
                new_message_count += 1;

                if msg.git_branch.is_some() {
                    latest_branch = msg.git_branch.clone();
                }
            }

            // Update session branch if messages show a different branch
            if let Some(ref new_branch) = latest_branch {
                if session.git_branch.as_ref() != Some(new_branch) {
                    if let Err(e) = db.update_session_branch(session.id, new_branch) {
                        tracing::warn!(
                            "Failed to update session branch for {}: {}",
                            &session.id.to_string()[..8],
                            e
                        );
                    } else {
                        tracing::debug!(
                            "Updated session {} branch to {}",
                            &session.id.to_string()[..8],
                            new_branch
                        );
                    }
                }
            }

            total_messages += new_message_count;
            updated_session = Some(session);
        }

        // Run auto-linking if the session has ended
        // This is the key fix: we now run auto-linking for updated sessions
        if let Some(session) = updated_session {
            if let Some(ended_at) = session.ended_at {
                // Only log if session was previously ongoing
                if existing_session.ended_at.is_none() {
                    tracing::info!(
                        "Session {} has ended, running auto-link",
                        &session.id.to_string()[..8]
                    );
                }

                let linked = self.auto_link_session_commits(
                    db,
                    session.id,
                    &session.working_directory,
                    session.started_at,
                    ended_at,
                );
                if let Err(e) = linked {
                    tracing::warn!(
                        "Failed to auto-link commits for session {}: {}",
                        &session.id.to_string()[..8],
                        e
                    );
                }
            }
        }

        // Return 0 sessions since we updated an existing one, not imported new
        Ok((0, total_messages))
    }

    /// Imports a complete session file synchronously.
    /// Returns (sessions_imported, messages_imported) counts.
    ///
    /// Uses path-based dispatch to find the correct watcher for the file.
    /// Files are parsed only by the watcher that owns their parent directory,
    /// preventing incorrect parsing by unrelated watchers.
    fn import_file_sync(&mut self, path: &Path, db: &Database) -> Result<(u64, u64)> {
        tracing::debug!("Importing session file: {:?}", path);

        let path_buf = path.to_path_buf();
        let registry = default_registry();
        let available = registry.available_watchers();

        // Find the watcher that owns this path
        let owning_watcher = match Self::find_owning_watcher(path, &available) {
            Some(w) => w,
            None => {
                tracing::debug!("No watcher owns path {:?}", path);
                return Ok((0, 0));
            }
        };

        // Parse with the owning watcher
        let parsed_sessions = match owning_watcher.parse_source(&path_buf) {
            Ok(sessions) => sessions,
            Err(e) => {
                tracing::debug!(
                    "Watcher {} could not parse {:?}: {}",
                    owning_watcher.info().name,
                    path,
                    e
                );
                return Ok((0, 0));
            }
        };

        if parsed_sessions.is_empty() {
            tracing::debug!(
                "Watcher {} returned no sessions for {:?}",
                owning_watcher.info().name,
                path
            );
            return Ok((0, 0));
        }

        let mut total_sessions = 0u64;
        let mut total_messages = 0u64;

        for (session, messages) in parsed_sessions {
            if messages.is_empty() {
                continue;
            }

            let message_count = messages.len();

            // Store session
            db.insert_session(&session)?;

            // Store messages and track the most recent branch
            let mut latest_branch: Option<String> = None;
            for msg in &messages {
                db.insert_message(msg)?;
                // Track the branch from the most recent message that has one
                if msg.git_branch.is_some() {
                    latest_branch = msg.git_branch.clone();
                }
            }

            // Update session branch if the latest message has a different branch
            // This handles the case where the user switches branches mid-session
            if let Some(ref new_branch) = latest_branch {
                if session.git_branch.as_ref() != Some(new_branch) {
                    if let Err(e) = db.update_session_branch(session.id, new_branch) {
                        tracing::warn!(
                            "Failed to update session branch for {}: {}",
                            &session.id.to_string()[..8],
                            e
                        );
                    } else {
                        tracing::debug!(
                            "Updated session {} branch to {}",
                            &session.id.to_string()[..8],
                            new_branch
                        );
                    }
                }
            }

            tracing::info!(
                "Imported session {} with {} messages from {:?}",
                &session.id.to_string()[..8],
                message_count,
                path.file_name().unwrap_or_default()
            );

            // Auto-link commits if session has ended
            if let Some(ended_at) = session.ended_at {
                let linked = self.auto_link_session_commits(
                    db,
                    session.id,
                    &session.working_directory,
                    session.started_at,
                    ended_at,
                );
                if let Err(e) = linked {
                    tracing::warn!(
                        "Failed to auto-link commits for session {}: {}",
                        &session.id.to_string()[..8],
                        e
                    );
                }
            }

            total_sessions += 1;
            total_messages += message_count as u64;
        }

        // Update file position
        if let Ok(metadata) = std::fs::metadata(path) {
            self.file_positions
                .insert(path.to_path_buf(), metadata.len());
        }

        Ok((total_sessions, total_messages))
    }

    /// Auto-links commits made during a session's time window.
    ///
    /// Finds all commits in the session's working directory that were made
    /// between the session's start and end time, then creates links for any
    /// commits that are not already linked to this session.
    ///
    /// # Arguments
    ///
    /// * `db` - Database connection for storing links
    /// * `session_id` - The session to link commits to
    /// * `working_directory` - Path to the repository working directory
    /// * `started_at` - Session start time
    /// * `ended_at` - Session end time
    ///
    /// # Returns
    ///
    /// The number of commits that were linked.
    fn auto_link_session_commits(
        &self,
        db: &Database,
        session_id: Uuid,
        working_directory: &str,
        started_at: chrono::DateTime<Utc>,
        ended_at: chrono::DateTime<Utc>,
    ) -> Result<usize> {
        let working_dir = Path::new(working_directory);

        // Check if working directory is a git repository
        if !working_dir.exists() {
            tracing::debug!("Working directory does not exist: {}", working_directory);
            return Ok(0);
        }

        // Get commits in the session's time range
        let commits = match get_commits_in_time_range(working_dir, started_at, ended_at) {
            Ok(commits) => commits,
            Err(e) => {
                // Not a git repository or other git error - this is expected
                // for sessions outside git repos
                tracing::debug!("Could not get commits for {}: {}", working_directory, e);
                return Ok(0);
            }
        };

        if commits.is_empty() {
            tracing::debug!(
                "No commits found in time range for session {}",
                &session_id.to_string()[..8]
            );
            return Ok(0);
        }

        let mut linked_count = 0;

        for commit in commits {
            // Skip if link already exists
            if db.link_exists(&session_id, &commit.sha)? {
                tracing::debug!(
                    "Link already exists for session {} and commit {}",
                    &session_id.to_string()[..8],
                    &commit.sha[..8]
                );
                continue;
            }

            // Create the link
            let link = SessionLink {
                id: Uuid::new_v4(),
                session_id,
                link_type: LinkType::Commit,
                commit_sha: Some(commit.sha.clone()),
                branch: commit.branch.clone(),
                remote: None,
                created_at: Utc::now(),
                created_by: LinkCreator::Auto,
                confidence: Some(1.0), // Direct time match is high confidence
            };

            db.insert_link(&link)?;
            linked_count += 1;

            tracing::info!(
                "Auto-linked commit {} to session {} ({})",
                &commit.sha[..8],
                &session_id.to_string()[..8],
                commit.summary.chars().take(50).collect::<String>()
            );
        }

        if linked_count > 0 {
            tracing::info!(
                "Auto-linked {} commits to session {}",
                linked_count,
                &session_id.to_string()[..8]
            );
        }

        Ok(linked_count)
    }

    /// Returns the number of files currently being tracked.
    ///
    /// This method is part of the public API for status reporting
    /// and is used by tests.
    #[allow(dead_code)]
    pub fn tracked_file_count(&self) -> usize {
        self.file_positions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::Session;
    use chrono::Duration;
    use tempfile::tempdir;

    /// Creates a test database in the given directory.
    fn create_test_db(dir: &Path) -> Database {
        let db_path = dir.join("test.db");
        Database::open(&db_path).expect("Failed to open test database")
    }

    /// Creates a test session with specified time window.
    fn create_test_session_with_times(
        working_directory: &str,
        started_at: chrono::DateTime<Utc>,
        ended_at: chrono::DateTime<Utc>,
    ) -> Session {
        Session {
            id: Uuid::new_v4(),
            tool: "test-tool".to_string(),
            tool_version: Some("1.0.0".to_string()),
            started_at,
            ended_at: Some(ended_at),
            model: Some("test-model".to_string()),
            working_directory: working_directory.to_string(),
            git_branch: Some("main".to_string()),
            source_path: None,
            message_count: 0,
            machine_id: Some("test-machine".to_string()),
        }
    }

    /// Creates a test commit in the repository with a specific timestamp.
    ///
    /// Returns the full SHA of the created commit.
    fn create_test_commit(
        repo: &git2::Repository,
        message: &str,
        time: chrono::DateTime<Utc>,
    ) -> String {
        let sig = git2::Signature::new(
            "Test User",
            "test@example.com",
            &git2::Time::new(time.timestamp(), 0),
        )
        .expect("Failed to create signature");

        // Get current tree (or create empty tree for first commit)
        let tree_id = {
            let mut index = repo.index().expect("Failed to get index");

            // Create a test file to have something to commit
            let file_path = repo
                .workdir()
                .unwrap()
                .join(format!("test_{}.txt", Uuid::new_v4()));
            std::fs::write(&file_path, format!("Content for: {message}"))
                .expect("Failed to write test file");

            index
                .add_path(file_path.strip_prefix(repo.workdir().unwrap()).unwrap())
                .expect("Failed to add file to index");
            index.write().expect("Failed to write index");
            index.write_tree().expect("Failed to write tree")
        };

        let tree = repo.find_tree(tree_id).expect("Failed to find tree");

        // Get parent commit if it exists
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

        let commit_id = if let Some(parent) = parent {
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
                .expect("Failed to create commit")
        } else {
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])
                .expect("Failed to create commit")
        };

        commit_id.to_string()
    }

    /// Initializes a new git repository in the given directory.
    fn init_test_repo(dir: &Path) -> git2::Repository {
        git2::Repository::init(dir).expect("Failed to init test repo")
    }

    #[test]
    fn test_session_watcher_creation() {
        let watcher = SessionWatcher::new();
        assert!(watcher.is_ok(), "Should create watcher successfully");

        // SessionWatcher creation should succeed even if no watchers are
        // available (e.g., in CI environments where ~/.claude doesn't exist).
        // The watch_dirs list may be empty in such environments.
        let _watcher = watcher.unwrap();
    }

    #[test]
    fn test_watch_dirs_from_registry() {
        use crate::capture::watchers::default_registry;

        // Test that the registry is configured with known watcher paths.
        // Note: SessionWatcher.watch_dirs() only includes paths from AVAILABLE
        // watchers. In CI environments, no watchers may be available because
        // their directories (like ~/.claude) don't exist.

        // Instead of testing through SessionWatcher, we verify the registry
        // directly by checking all_watchers() (not just available ones).
        let registry = default_registry();
        let all_watchers = registry.all_watchers();

        // Collect watch paths from ALL watchers (including unavailable ones)
        let all_paths: Vec<_> = all_watchers.iter().flat_map(|w| w.watch_paths()).collect();

        let has_claude = all_paths
            .iter()
            .any(|d| d.to_string_lossy().contains(".claude"));
        let has_cursor = all_paths
            .iter()
            .any(|d| d.to_string_lossy().contains("Cursor"));

        // The registry should have paths configured for known watchers.
        assert!(
            has_claude || has_cursor,
            "Registry should configure at least one known watcher path pattern \
             (expected .claude or Cursor in paths). Found paths: {all_paths:?}"
        );
    }

    #[test]
    fn test_tracked_file_count_initial() {
        let watcher = SessionWatcher::new().unwrap();
        assert_eq!(
            watcher.tracked_file_count(),
            0,
            "Should start with no tracked files"
        );
    }

    #[test]
    fn test_db_config_creation() {
        let config = DbConfig::default_config();
        assert!(config.is_ok(), "Should create DbConfig successfully");
    }

    #[test]
    fn test_file_position_tracking() {
        let mut watcher = SessionWatcher::new().unwrap();

        let path1 = PathBuf::from("/test/file1.jsonl");
        let path2 = PathBuf::from("/test/file2.jsonl");

        watcher.file_positions.insert(path1.clone(), 100);
        watcher.file_positions.insert(path2.clone(), 200);

        assert_eq!(watcher.tracked_file_count(), 2);
        assert_eq!(watcher.file_positions.get(&path1), Some(&100));
        assert_eq!(watcher.file_positions.get(&path2), Some(&200));
    }

    // ==================== find_owning_watcher Tests ====================

    #[test]
    fn test_find_owning_watcher_matches_path_under_watch_dir() {
        use crate::capture::watchers::{Watcher, WatcherInfo};
        use crate::storage::models::{Message, Session};

        struct TestWatcher {
            name: &'static str,
            watch_path: PathBuf,
        }

        impl Watcher for TestWatcher {
            fn info(&self) -> WatcherInfo {
                WatcherInfo {
                    name: self.name,
                    description: "Test",
                    default_paths: vec![],
                }
            }
            fn is_available(&self) -> bool {
                true
            }
            fn find_sources(&self) -> Result<Vec<PathBuf>> {
                Ok(vec![])
            }
            fn parse_source(&self, _: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
                Ok(vec![])
            }
            fn watch_paths(&self) -> Vec<PathBuf> {
                vec![self.watch_path.clone()]
            }
        }

        let watcher1 = TestWatcher {
            name: "watcher-a",
            watch_path: PathBuf::from("/home/user/.claude/projects"),
        };
        let watcher2 = TestWatcher {
            name: "watcher-b",
            watch_path: PathBuf::from("/home/user/.aider"),
        };

        let watchers: Vec<&dyn Watcher> = vec![&watcher1, &watcher2];

        // File under watcher1's path
        let claude_file = Path::new("/home/user/.claude/projects/myproject/session.jsonl");
        let result = SessionWatcher::find_owning_watcher(claude_file, &watchers);
        assert!(result.is_some());
        assert_eq!(result.unwrap().info().name, "watcher-a");

        // File under watcher2's path
        let aider_file = Path::new("/home/user/.aider/history.md");
        let result = SessionWatcher::find_owning_watcher(aider_file, &watchers);
        assert!(result.is_some());
        assert_eq!(result.unwrap().info().name, "watcher-b");

        // File not under any watch path
        let other_file = Path::new("/home/user/projects/random.txt");
        let result = SessionWatcher::find_owning_watcher(other_file, &watchers);
        assert!(result.is_none());
    }

    // ==================== auto_link_session_commits Tests ====================

    #[test]
    fn test_auto_link_creates_links_for_commits_in_time_range() {
        // Create temp directory for both git repo and database
        let dir = tempdir().expect("Failed to create temp directory");
        let repo_path = dir.path();

        // Initialize git repo
        let repo = init_test_repo(repo_path);

        // Create database in the temp directory
        let db = create_test_db(repo_path);

        // Define time window for session: 1 hour ago to now
        let now = Utc::now();
        let session_start = now - Duration::hours(1);
        let session_end = now;

        // Create commits within the session's time window
        let commit_time1 = session_start + Duration::minutes(10);
        let commit_time2 = session_start + Duration::minutes(30);
        let commit_time3 = session_start + Duration::minutes(50);

        let sha1 = create_test_commit(&repo, "First commit in session", commit_time1);
        let sha2 = create_test_commit(&repo, "Second commit in session", commit_time2);
        let sha3 = create_test_commit(&repo, "Third commit in session", commit_time3);

        // Create and insert session
        let session = create_test_session_with_times(
            &repo_path.to_string_lossy(),
            session_start,
            session_end,
        );
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Create a minimal watcher for testing
        let watcher = SessionWatcher {
            file_positions: HashMap::new(),
            watch_dirs: vec![],
            db_config: DbConfig {
                path: repo_path.join("test.db"),
            },
        };

        // Call auto_link_session_commits
        let linked_count = watcher
            .auto_link_session_commits(
                &db,
                session.id,
                &repo_path.to_string_lossy(),
                session_start,
                session_end,
            )
            .expect("auto_link_session_commits should succeed");

        // Assert correct number of links created
        assert_eq!(linked_count, 3, "Should have linked 3 commits");

        // Verify each commit is linked
        assert!(
            db.link_exists(&session.id, &sha1)
                .expect("link_exists should succeed"),
            "First commit should be linked"
        );
        assert!(
            db.link_exists(&session.id, &sha2)
                .expect("link_exists should succeed"),
            "Second commit should be linked"
        );
        assert!(
            db.link_exists(&session.id, &sha3)
                .expect("link_exists should succeed"),
            "Third commit should be linked"
        );
    }

    #[test]
    fn test_auto_link_skips_commits_outside_time_range() {
        // Create temp directory
        let dir = tempdir().expect("Failed to create temp directory");
        let repo_path = dir.path();

        // Initialize git repo
        let repo = init_test_repo(repo_path);

        // Create database
        let db = create_test_db(repo_path);

        // Define a narrow time window: 30 minutes to 20 minutes ago
        let now = Utc::now();
        let session_start = now - Duration::minutes(30);
        let session_end = now - Duration::minutes(20);

        // Create commits BEFORE the session window (40 minutes ago)
        let before_time = now - Duration::minutes(40);
        let sha_before = create_test_commit(&repo, "Commit before session", before_time);

        // Create commit INSIDE the session window (25 minutes ago)
        let inside_time = now - Duration::minutes(25);
        let sha_inside = create_test_commit(&repo, "Commit inside session", inside_time);

        // Create commit AFTER the session window (10 minutes ago)
        let after_time = now - Duration::minutes(10);
        let sha_after = create_test_commit(&repo, "Commit after session", after_time);

        // Create and insert session
        let session = create_test_session_with_times(
            &repo_path.to_string_lossy(),
            session_start,
            session_end,
        );
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Create watcher and call auto_link
        let watcher = SessionWatcher {
            file_positions: HashMap::new(),
            watch_dirs: vec![],
            db_config: DbConfig {
                path: repo_path.join("test.db"),
            },
        };

        let linked_count = watcher
            .auto_link_session_commits(
                &db,
                session.id,
                &repo_path.to_string_lossy(),
                session_start,
                session_end,
            )
            .expect("auto_link_session_commits should succeed");

        // Only the commit inside the window should be linked
        assert_eq!(linked_count, 1, "Should have linked only 1 commit");

        // Verify the commit before is NOT linked
        assert!(
            !db.link_exists(&session.id, &sha_before)
                .expect("link_exists should succeed"),
            "Commit before session should NOT be linked"
        );

        // Verify the commit inside IS linked
        assert!(
            db.link_exists(&session.id, &sha_inside)
                .expect("link_exists should succeed"),
            "Commit inside session should be linked"
        );

        // Verify the commit after is NOT linked
        assert!(
            !db.link_exists(&session.id, &sha_after)
                .expect("link_exists should succeed"),
            "Commit after session should NOT be linked"
        );
    }

    #[test]
    fn test_auto_link_skips_existing_links() {
        // Create temp directory
        let dir = tempdir().expect("Failed to create temp directory");
        let repo_path = dir.path();

        // Initialize git repo
        let repo = init_test_repo(repo_path);

        // Create database
        let db = create_test_db(repo_path);

        // Define time window
        let now = Utc::now();
        let session_start = now - Duration::hours(1);
        let session_end = now;

        // Create a commit in the time window
        let commit_time = session_start + Duration::minutes(30);
        let sha = create_test_commit(&repo, "Test commit", commit_time);

        // Create and insert session
        let session = create_test_session_with_times(
            &repo_path.to_string_lossy(),
            session_start,
            session_end,
        );
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Manually create a link for the commit
        let existing_link = SessionLink {
            id: Uuid::new_v4(),
            session_id: session.id,
            link_type: LinkType::Commit,
            commit_sha: Some(sha.clone()),
            branch: Some("main".to_string()),
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::Auto,
            confidence: Some(1.0),
        };
        db.insert_link(&existing_link)
            .expect("Failed to insert existing link");

        // Create watcher and call auto_link
        let watcher = SessionWatcher {
            file_positions: HashMap::new(),
            watch_dirs: vec![],
            db_config: DbConfig {
                path: repo_path.join("test.db"),
            },
        };

        let linked_count = watcher
            .auto_link_session_commits(
                &db,
                session.id,
                &repo_path.to_string_lossy(),
                session_start,
                session_end,
            )
            .expect("auto_link_session_commits should succeed");

        // Should return 0 since the link already exists
        assert_eq!(
            linked_count, 0,
            "Should not create any new links when link already exists"
        );

        // Verify the link still exists (and there is only one)
        assert!(
            db.link_exists(&session.id, &sha)
                .expect("link_exists should succeed"),
            "Link should still exist"
        );
    }

    #[test]
    fn test_auto_link_handles_non_git_directory() {
        // Create temp directory that is NOT a git repo
        let dir = tempdir().expect("Failed to create temp directory");
        let non_repo_path = dir.path();

        // Create database
        let db = create_test_db(non_repo_path);

        // Define time window
        let now = Utc::now();
        let session_start = now - Duration::hours(1);
        let session_end = now;

        // Create and insert session pointing to non-git directory
        let session = create_test_session_with_times(
            &non_repo_path.to_string_lossy(),
            session_start,
            session_end,
        );
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Create watcher and call auto_link
        let watcher = SessionWatcher {
            file_positions: HashMap::new(),
            watch_dirs: vec![],
            db_config: DbConfig {
                path: non_repo_path.join("test.db"),
            },
        };

        let result = watcher.auto_link_session_commits(
            &db,
            session.id,
            &non_repo_path.to_string_lossy(),
            session_start,
            session_end,
        );

        // Should return Ok(0), not an error
        assert!(
            result.is_ok(),
            "Should handle non-git directory gracefully: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap(), 0, "Should return 0 for non-git directory");
    }

    #[test]
    fn test_auto_link_finds_commits_on_multiple_branches() {
        // Create temp directory
        let dir = tempdir().expect("Failed to create temp directory");
        let repo_path = dir.path();

        // Initialize git repo
        let repo = init_test_repo(repo_path);

        // Create database
        let db = create_test_db(repo_path);

        // Define time window: 1 hour ago to now
        let now = Utc::now();
        let session_start = now - Duration::hours(1);
        let session_end = now;

        // Create initial commit on main branch (default)
        let main_commit_time = session_start + Duration::minutes(10);
        let sha_main = create_test_commit(&repo, "Commit on main", main_commit_time);

        // Get the default branch name (could be master or main depending on git config)
        let head_ref = repo.head().expect("Should have HEAD after commit");
        let default_branch = head_ref
            .shorthand()
            .expect("HEAD should have a name")
            .to_string();

        // Create a feature branch and switch to it
        let main_commit = head_ref.peel_to_commit().unwrap();
        repo.branch("feature-branch", &main_commit, false)
            .expect("Failed to create branch");
        repo.set_head("refs/heads/feature-branch")
            .expect("Failed to switch branch");

        // Create commit on feature branch
        let feature_commit_time = session_start + Duration::minutes(30);
        let sha_feature = create_test_commit(&repo, "Commit on feature", feature_commit_time);

        // Switch back to default branch and create another commit
        repo.set_head(&format!("refs/heads/{}", default_branch))
            .expect("Failed to switch to default branch");
        // Need to reset the working directory to default branch
        let main_obj = repo
            .revparse_single(&default_branch)
            .expect("Should find default branch");
        repo.reset(&main_obj, git2::ResetType::Hard, None)
            .expect("Failed to reset to default branch");

        let main_commit_time2 = session_start + Duration::minutes(50);
        let sha_main2 = create_test_commit(&repo, "Second commit on main", main_commit_time2);

        // Create and insert session
        let session = create_test_session_with_times(
            &repo_path.to_string_lossy(),
            session_start,
            session_end,
        );
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Create watcher and call auto_link
        let watcher = SessionWatcher {
            file_positions: HashMap::new(),
            watch_dirs: vec![],
            db_config: DbConfig {
                path: repo_path.join("test.db"),
            },
        };

        let linked_count = watcher
            .auto_link_session_commits(
                &db,
                session.id,
                &repo_path.to_string_lossy(),
                session_start,
                session_end,
            )
            .expect("auto_link_session_commits should succeed");

        // Should link all 3 commits from both branches
        assert_eq!(
            linked_count, 3,
            "Should have linked commits from both branches"
        );

        // Verify each commit is linked
        assert!(
            db.link_exists(&session.id, &sha_main)
                .expect("link_exists should succeed"),
            "First main commit should be linked"
        );
        assert!(
            db.link_exists(&session.id, &sha_feature)
                .expect("link_exists should succeed"),
            "Feature branch commit should be linked"
        );
        assert!(
            db.link_exists(&session.id, &sha_main2)
                .expect("link_exists should succeed"),
            "Second main commit should be linked"
        );
    }

    #[test]
    fn test_update_existing_session_triggers_auto_link() {
        // This test verifies that when a session already exists in the database
        // and the file grows (new messages), re-importing triggers auto-linking.

        // Create temp directory
        let dir = tempdir().expect("Failed to create temp directory");
        let repo_path = dir.path();

        // Initialize git repo and create database
        let repo = init_test_repo(repo_path);
        let db = create_test_db(repo_path);

        // Define time window
        let now = Utc::now();
        let session_start = now - Duration::hours(1);
        let session_end = now;

        // Create a commit during the session
        let commit_time = session_start + Duration::minutes(30);
        let sha = create_test_commit(&repo, "Commit during session", commit_time);

        // Create a session WITHOUT ended_at (simulating ongoing session from CLI import)
        let session_id = Uuid::new_v4();
        let ongoing_session = Session {
            id: session_id,
            tool: "test-tool".to_string(),
            tool_version: Some("1.0.0".to_string()),
            started_at: session_start,
            ended_at: None, // Not ended yet
            model: Some("test-model".to_string()),
            working_directory: repo_path.to_string_lossy().to_string(),
            git_branch: Some("main".to_string()),
            source_path: Some("/test/session.jsonl".to_string()),
            message_count: 5,
            machine_id: Some("test-machine".to_string()),
        };

        db.insert_session(&ongoing_session)
            .expect("Failed to insert session");

        // Verify commit is NOT linked yet (since session has not ended)
        assert!(
            !db.link_exists(&session_id, &sha)
                .expect("link_exists should succeed"),
            "Commit should NOT be linked to ongoing session"
        );

        // Now create an updated session with ended_at set
        let ended_session = Session {
            id: session_id,
            tool: "test-tool".to_string(),
            tool_version: Some("1.0.0".to_string()),
            started_at: session_start,
            ended_at: Some(session_end), // Now ended
            model: Some("test-model".to_string()),
            working_directory: repo_path.to_string_lossy().to_string(),
            git_branch: Some("main".to_string()),
            source_path: Some("/test/session.jsonl".to_string()),
            message_count: 10,
            machine_id: Some("test-machine".to_string()),
        };

        // Create watcher
        let watcher = SessionWatcher {
            file_positions: HashMap::new(),
            watch_dirs: vec![],
            db_config: DbConfig {
                path: repo_path.join("test.db"),
            },
        };

        // Simulate what update_existing_session does:
        // 1. Update session in DB
        db.insert_session(&ended_session)
            .expect("Failed to update session");

        // 2. Run auto-linking (this is what the fix enables)
        let linked_count = watcher
            .auto_link_session_commits(
                &db,
                session_id,
                &repo_path.to_string_lossy(),
                session_start,
                session_end,
            )
            .expect("auto_link_session_commits should succeed");

        // Verify the commit is now linked
        assert_eq!(linked_count, 1, "Should have linked 1 commit");
        assert!(
            db.link_exists(&session_id, &sha)
                .expect("link_exists should succeed"),
            "Commit should be linked after session ended"
        );
    }
}

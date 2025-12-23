//! File system watcher for AI tool session files.
//!
//! Watches directories configured by the WatcherRegistry for new and modified
//! session files. Performs incremental parsing to efficiently handle file
//! updates without re-reading entire files.
//!
//! Currently supports:
//! - Claude Code JSONL files in `~/.claude/projects/`
//! - Cursor SQLite databases in workspace storage

use anyhow::{Context, Result};
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

use crate::capture::watchers::default_registry;
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
                tracing::info!(
                    "Watch directory does not exist yet: {:?}",
                    dir
                );
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
                            let ext = e.path
                                .extension()
                                .and_then(|ext| ext.to_str());
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
                            tracing::warn!("Error handling file event for {:?}: {}", event.path, e);
                            let mut stats_guard = stats.write().await;
                            stats_guard.errors += 1;
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

    /// Performs an initial scan of existing session files.
    ///
    /// Called when the watcher starts to import any sessions that were
    /// created while the daemon was not running. Uses the watcher registry
    /// to find session sources from all available watchers.
    async fn initial_scan(
        &mut self,
        stats: &Arc<RwLock<DaemonStats>>,
    ) -> Result<()> {
        tracing::info!("Performing initial scan of session files...");

        let registry = default_registry();
        let mut total_files = 0;

        for watcher in registry.available_watchers() {
            let watcher_name = watcher.info().name;
            match watcher.find_sources() {
                Ok(sources) => {
                    tracing::info!(
                        "Found {} sources for {}",
                        sources.len(),
                        watcher_name
                    );
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
    fn process_file_sync(&mut self, path: &Path) -> Result<Option<(u64, u64)>> {
        let db = self.open_db()?;
        let path_str = path.to_string_lossy();
        let last_pos = self.file_positions.get(path).copied().unwrap_or(0);

        // Get current file size
        let metadata = std::fs::metadata(path)
            .context("Failed to get file metadata")?;
        let current_size = metadata.len();

        if current_size <= last_pos {
            // File hasn't grown (might have been truncated)
            if current_size < last_pos {
                // File was truncated, reset position
                self.file_positions.insert(path.to_path_buf(), 0);
            }
            return Ok(None);
        }

        // Check if this is a new file we haven't seen
        if db.session_exists_by_source(&path_str)? {
            // Already imported, just update position
            self.file_positions.insert(path.to_path_buf(), current_size);
            return Ok(None);
        }

        // Import the file
        let result = self.import_file_sync(path, &db)?;

        // Update tracked position
        self.file_positions.insert(path.to_path_buf(), current_size);

        Ok(Some(result))
    }

    /// Imports a complete session file synchronously.
    /// Returns (sessions_imported, messages_imported) counts.
    ///
    /// Uses the watcher registry to find the appropriate parser for the file type.
    fn import_file_sync(&mut self, path: &Path, db: &Database) -> Result<(u64, u64)> {
        tracing::debug!("Importing session file: {:?}", path);

        let path_buf = path.to_path_buf();
        let registry = default_registry();

        // Try to parse with each watcher until one succeeds
        let mut parsed_sessions = Vec::new();

        for watcher in registry.available_watchers() {
            match watcher.parse_source(&path_buf) {
                Ok(sessions) if !sessions.is_empty() => {
                    parsed_sessions = sessions;
                    break;
                }
                Ok(_) => continue,
                Err(e) => {
                    tracing::debug!(
                        "Watcher {} could not parse {:?}: {}",
                        watcher.info().name,
                        path,
                        e
                    );
                }
            }
        }

        if parsed_sessions.is_empty() {
            tracing::debug!("No watcher could parse {:?}", path);
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

            // Store messages
            for msg in &messages {
                db.insert_message(msg)?;
            }

            tracing::info!(
                "Imported session {} with {} messages from {:?}",
                &session.id.to_string()[..8],
                message_count,
                path.file_name().unwrap_or_default()
            );

            total_sessions += 1;
            total_messages += message_count as u64;
        }

        // Update file position
        if let Ok(metadata) = std::fs::metadata(path) {
            self.file_positions.insert(path.to_path_buf(), metadata.len());
        }

        Ok((total_sessions, total_messages))
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

    #[test]
    fn test_session_watcher_creation() {
        let watcher = SessionWatcher::new();
        assert!(watcher.is_ok(), "Should create watcher successfully");

        let watcher = watcher.unwrap();
        // Should have watch directories configured from the registry
        let dirs = watcher.watch_dirs();
        assert!(
            !dirs.is_empty(),
            "Should have at least one watch directory configured"
        );
    }

    #[test]
    fn test_watch_dirs_from_registry() {
        let watcher = SessionWatcher::new().unwrap();
        let dirs = watcher.watch_dirs();

        // Should include paths from both Claude Code and Cursor watchers
        let has_claude = dirs.iter().any(|d| d.to_string_lossy().contains(".claude"));
        let has_cursor = dirs.iter().any(|d| d.to_string_lossy().contains("Cursor"));

        assert!(has_claude || has_cursor, "Should have at least one known watcher path");
    }

    #[test]
    fn test_tracked_file_count_initial() {
        let watcher = SessionWatcher::new().unwrap();
        assert_eq!(watcher.tracked_file_count(), 0, "Should start with no tracked files");
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
}

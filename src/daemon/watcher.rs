//! File system watcher for Claude Code session files.
//!
//! Watches `~/.claude/projects/` for new and modified JSONL session files.
//! Performs incremental parsing to efficiently handle file updates without
//! re-reading entire files.

use anyhow::{Context, Result};
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

use crate::capture::watchers::claude_code;
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
    /// The directory to watch for session files.
    watch_dir: PathBuf,
    /// Database configuration for creating connections.
    db_config: DbConfig,
}

impl SessionWatcher {
    /// Creates a new SessionWatcher.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined.
    pub fn new() -> Result<Self> {
        let watch_dir = dirs::home_dir()
            .context("Could not find home directory")?
            .join(".claude")
            .join("projects");

        let db_config = DbConfig::default_config()?;

        Ok(Self {
            file_positions: HashMap::new(),
            watch_dir,
            db_config,
        })
    }

    /// Returns the directory being watched.
    ///
    /// This method is part of the public API for status reporting
    /// and may be used by CLI commands in the future.
    #[allow(dead_code)]
    pub fn watch_dir(&self) -> &Path {
        &self.watch_dir
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
        // Check if watch directory exists
        if !self.watch_dir.exists() {
            tracing::info!(
                "Claude Code directory does not exist yet: {:?}",
                self.watch_dir
            );
            tracing::info!("Watcher will wait for directory to be created...");
        }

        // Create a channel for file events
        let (tx, mut rx) = mpsc::channel::<Vec<DebouncedEvent>>(100);

        // Create the debounced watcher
        let mut debouncer = new_debouncer(
            Duration::from_millis(500),
            move |events: Result<Vec<DebouncedEvent>, notify::Error>| {
                if let Ok(events) = events {
                    // Filter for JSONL files
                    let filtered: Vec<DebouncedEvent> = events
                        .into_iter()
                        .filter(|e| {
                            e.path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .map(|ext| ext == "jsonl")
                                .unwrap_or(false)
                        })
                        .collect();

                    if !filtered.is_empty() {
                        let _ = tx.blocking_send(filtered);
                    }
                }
            },
        )
        .context("Failed to create file watcher")?;

        // Start watching if directory exists
        if self.watch_dir.exists() {
            debouncer
                .watcher()
                .watch(&self.watch_dir, RecursiveMode::Recursive)
                .context("Failed to start watching directory")?;

            tracing::info!("Watching for session files in {:?}", self.watch_dir);

            // Do an initial scan (sync, before entering async loop)
            self.initial_scan(&stats).await?;
        }

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
    /// created while the daemon was not running.
    async fn initial_scan(
        &mut self,
        stats: &Arc<RwLock<DaemonStats>>,
    ) -> Result<()> {
        tracing::info!("Performing initial scan of session files...");

        let session_files = claude_code::find_session_files()?;

        {
            let mut stats_guard = stats.write().await;
            stats_guard.files_watched = session_files.len();
        }

        for path in session_files {
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

        Ok(())
    }

    /// Handles a file system event for a session file.
    async fn handle_file_event(
        &mut self,
        path: &Path,
        stats: &Arc<RwLock<DaemonStats>>,
    ) -> Result<()> {
        // Skip non-JSONL files
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            return Ok(());
        }

        // Skip agent files
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
    fn import_file_sync(&mut self, path: &Path, db: &Database) -> Result<(u64, u64)> {
        tracing::debug!("Importing session file: {:?}", path);

        let parsed = claude_code::parse_session_file(path)?;

        if parsed.messages.is_empty() {
            tracing::debug!("Skipping empty session: {:?}", path);
            return Ok((0, 0));
        }

        let (session, messages) = parsed.to_storage_models();
        let message_count = messages.len();

        // Store session
        db.insert_session(&session)?;

        // Store messages
        for msg in &messages {
            db.insert_message(msg)?;
        }

        // Update file position
        if let Ok(metadata) = std::fs::metadata(path) {
            self.file_positions.insert(path.to_path_buf(), metadata.len());
        }

        tracing::info!(
            "Imported session {} with {} messages from {:?}",
            &session.id.to_string()[..8],
            message_count,
            path.file_name().unwrap_or_default()
        );

        Ok((1, message_count as u64))
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
        assert!(
            watcher.watch_dir().to_string_lossy().contains(".claude"),
            "Watch dir should be in .claude directory"
        );
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

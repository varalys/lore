//! Periodic cloud sync for the daemon.
//!
//! Provides automatic synchronization of sessions to the cloud at regular
//! intervals. The sync timer checks for credentials and encryption key
//! availability before attempting to push pending sessions.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

use crate::cloud::client::{CloudClient, PushSession, SessionMetadata};
use crate::cloud::credentials::CredentialsStore;
use crate::cloud::encryption::{decode_key_hex, encode_base64, encrypt_data};
use crate::config::Config;
use crate::storage::models::Message;
use crate::storage::Database;

/// Default interval between automatic syncs (4 hours).
const SYNC_INTERVAL_HOURS: u64 = 4;

/// Number of sessions to include in each batch when pushing to the cloud.
const PUSH_BATCH_SIZE: usize = 3;

/// Persistent state for daemon sync scheduling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncState {
    /// When the last sync was performed (successfully or not).
    pub last_sync_at: Option<DateTime<Utc>>,
    /// When the next sync is scheduled.
    pub next_sync_at: Option<DateTime<Utc>>,
    /// Number of sessions synced in the last sync.
    pub last_sync_count: Option<u64>,
    /// Whether the last sync was successful.
    pub last_sync_success: Option<bool>,
}

impl SyncState {
    /// Returns the path to the sync state file.
    fn state_path() -> Result<PathBuf> {
        let lore_dir = dirs::home_dir()
            .context("Could not find home directory")?
            .join(".lore");
        Ok(lore_dir.join("daemon_state.json"))
    }

    /// Loads the sync state from disk.
    ///
    /// Returns the default state if the file does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::state_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).context("Failed to read sync state file")?;
        let state: SyncState =
            serde_json::from_str(&content).context("Failed to parse sync state file")?;
        Ok(state)
    }

    /// Saves the sync state to disk atomically.
    fn save(&self) -> Result<()> {
        let path = Self::state_path()?;
        let content = serde_json::to_string_pretty(self)?;

        // Write to a temp file first, then rename for atomicity
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, &content).context("Failed to write sync state temp file")?;
        fs::rename(&temp_path, &path).context("Failed to rename sync state file")?;

        Ok(())
    }

    /// Updates the state with next sync time and saves.
    fn schedule_next(&mut self, next_at: DateTime<Utc>) -> Result<()> {
        self.next_sync_at = Some(next_at);
        self.save()
    }

    /// Updates the state after a sync attempt and saves.
    fn record_sync(&mut self, success: bool, count: u64, next_at: DateTime<Utc>) -> Result<()> {
        self.last_sync_at = Some(Utc::now());
        self.last_sync_success = Some(success);
        self.last_sync_count = Some(count);
        self.next_sync_at = Some(next_at);
        self.save()
    }
}

/// Shared sync state for the daemon.
pub type SharedSyncState = Arc<RwLock<SyncState>>;

/// Calculates the next sync time based on the last sync.
///
/// If there was a previous sync, schedules the next one SYNC_INTERVAL_HOURS
/// after that. If not, schedules SYNC_INTERVAL_HOURS from now.
fn calculate_next_sync(state: &SyncState) -> DateTime<Utc> {
    let interval = chrono::Duration::hours(SYNC_INTERVAL_HOURS as i64);

    if let Some(last_sync) = state.last_sync_at {
        // Schedule from last sync + interval
        let next = last_sync + interval;
        // If that time has already passed, schedule from now
        let now = Utc::now();
        if next <= now {
            now + interval
        } else {
            next
        }
    } else {
        // No previous sync, schedule from now
        Utc::now() + interval
    }
}

/// Runs the periodic sync timer.
///
/// This function runs until the shutdown signal is received. It checks
/// periodically if a sync is needed and performs it if credentials and
/// encryption key are available.
pub async fn run_periodic_sync(
    sync_state: SharedSyncState,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    // Initialize state with next sync time
    {
        let mut state = sync_state.write().await;
        let next_sync = calculate_next_sync(&state);
        if let Err(e) = state.schedule_next(next_sync) {
            tracing::warn!("Failed to save initial sync state: {e}");
        } else {
            tracing::info!(
                "Periodic sync scheduled for {}",
                next_sync.format("%Y-%m-%d %H:%M:%S UTC")
            );
        }
    }

    // Check every minute if it's time to sync
    let mut check_interval = interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                let should_sync = {
                    let state = sync_state.read().await;
                    if let Some(next_sync) = state.next_sync_at {
                        Utc::now() >= next_sync
                    } else {
                        false
                    }
                };

                if should_sync {
                    let result = perform_sync().await;
                    let next_sync = Utc::now() + chrono::Duration::hours(SYNC_INTERVAL_HOURS as i64);

                    let mut state = sync_state.write().await;
                    match result {
                        Ok(count) => {
                            tracing::info!("Periodic sync completed: {} sessions synced", count);
                            if let Err(e) = state.record_sync(true, count, next_sync) {
                                tracing::warn!("Failed to save sync state: {e}");
                            }
                        }
                        Err(e) => {
                            tracing::info!("Periodic sync skipped or failed: {e}");
                            if let Err(e) = state.record_sync(false, 0, next_sync) {
                                tracing::warn!("Failed to save sync state: {e}");
                            }
                        }
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Periodic sync shutting down");
                break;
            }
        }
    }
}

/// Performs a sync operation, pushing pending sessions to the cloud.
///
/// Returns the number of sessions synced, or an error if sync cannot proceed
/// (e.g., not logged in, no encryption key).
async fn perform_sync() -> Result<u64> {
    // Load config to check login status and keychain preference
    let config = Config::load().context("Could not load config")?;

    // Create credentials store with user's preference
    let store = CredentialsStore::with_keychain(config.use_keychain);

    // Check if user is logged in
    let credentials = match store.load()? {
        Some(creds) => creds,
        None => {
            return Err(anyhow::anyhow!("Not logged in"));
        }
    };

    // Check if encryption key is available
    let encryption_key = match store.load_encryption_key()? {
        Some(key_hex) => decode_key_hex(&key_hex)?,
        None => {
            return Err(anyhow::anyhow!("Encryption key not configured"));
        }
    };

    // Get machine ID
    let machine_id = match config.machine_id.clone() {
        Some(id) => id,
        None => {
            return Err(anyhow::anyhow!("Machine ID not configured"));
        }
    };

    // Open database
    let db = Database::open_default().context("Could not open database")?;

    // Get unsynced sessions
    let sessions = db.get_unsynced_sessions()?;
    if sessions.is_empty() {
        tracing::debug!("No sessions to sync");
        return Ok(0);
    }

    tracing::info!("Found {} sessions to sync", sessions.len());

    // Create cloud client
    let client = CloudClient::with_url(&credentials.cloud_url).with_api_key(&credentials.api_key);

    // Prepare session data
    let session_data: Vec<_> = sessions
        .iter()
        .filter_map(|session| match db.get_messages(&session.id) {
            Ok(messages) => Some((session.clone(), messages)),
            Err(e) => {
                tracing::warn!(
                    "Failed to get messages for session {}: {}",
                    &session.id.to_string()[..8],
                    e
                );
                None
            }
        })
        .collect();

    // Process in batches
    let mut total_synced: u64 = 0;

    for batch in session_data.chunks(PUSH_BATCH_SIZE) {
        let mut push_sessions = Vec::new();

        for (session, messages) in batch {
            let encrypted = encrypt_session_messages(messages, &encryption_key)?;
            push_sessions.push(PushSession {
                id: session.id.to_string(),
                machine_id: machine_id.clone(),
                encrypted_data: encrypted,
                metadata: SessionMetadata {
                    tool_name: session.tool.clone(),
                    project_path: session.working_directory.clone(),
                    started_at: session.started_at,
                    ended_at: session.ended_at,
                    message_count: session.message_count,
                },
                updated_at: session.ended_at.unwrap_or_else(Utc::now),
            });
        }

        match client.push(push_sessions.clone()) {
            Ok(response) => {
                // Mark sessions in this batch as synced
                let batch_session_ids: Vec<_> = push_sessions
                    .iter()
                    .filter_map(|ps| uuid::Uuid::parse_str(&ps.id).ok())
                    .collect();

                if let Err(e) = db.mark_sessions_synced(&batch_session_ids, response.server_time) {
                    tracing::warn!("Failed to mark sessions as synced: {e}");
                }

                total_synced += response.synced_count as u64;
            }
            Err(e) => {
                let error_str = e.to_string();
                // Stop on quota errors
                if error_str.contains("quota")
                    || error_str.contains("Would exceed session limit")
                    || (error_str.contains("403") && error_str.contains("limit"))
                {
                    tracing::debug!("Sync stopped due to quota limit");
                    break;
                }
                tracing::warn!("Failed to push batch: {e}");
            }
        }
    }

    Ok(total_synced)
}

/// Encrypts session messages for cloud storage.
fn encrypt_session_messages(messages: &[Message], key: &[u8]) -> Result<String> {
    let json = serde_json::to_vec(messages)?;
    let encrypted = encrypt_data(&json, key)?;
    Ok(encode_base64(&encrypted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_state_default() {
        let state = SyncState::default();
        assert!(state.last_sync_at.is_none());
        assert!(state.next_sync_at.is_none());
        assert!(state.last_sync_count.is_none());
        assert!(state.last_sync_success.is_none());
    }

    #[test]
    fn test_calculate_next_sync_no_previous() {
        let state = SyncState::default();
        let next = calculate_next_sync(&state);

        // Should be approximately 4 hours from now
        let expected = Utc::now() + chrono::Duration::hours(SYNC_INTERVAL_HOURS as i64);
        let diff = (next - expected).num_seconds().abs();
        assert!(diff < 5, "Next sync should be ~4 hours from now");
    }

    #[test]
    fn test_calculate_next_sync_with_recent_previous() {
        let last_sync = Utc::now() - chrono::Duration::hours(1);
        let state = SyncState {
            last_sync_at: Some(last_sync),
            ..Default::default()
        };

        let next = calculate_next_sync(&state);

        // Should be 4 hours after the last sync (3 hours from now)
        let expected = last_sync + chrono::Duration::hours(SYNC_INTERVAL_HOURS as i64);
        let diff = (next - expected).num_seconds().abs();
        assert!(diff < 5, "Next sync should be 4 hours after last sync");
    }

    #[test]
    fn test_calculate_next_sync_with_old_previous() {
        // Last sync was 10 hours ago
        let state = SyncState {
            last_sync_at: Some(Utc::now() - chrono::Duration::hours(10)),
            ..Default::default()
        };

        let next = calculate_next_sync(&state);

        // Since last + interval is in the past, should be 4 hours from now
        let expected = Utc::now() + chrono::Duration::hours(SYNC_INTERVAL_HOURS as i64);
        let diff = (next - expected).num_seconds().abs();
        assert!(
            diff < 5,
            "Next sync should be ~4 hours from now when last sync is old"
        );
    }

    #[test]
    fn test_sync_state_serialization() {
        let state = SyncState {
            last_sync_at: Some(Utc::now()),
            next_sync_at: Some(Utc::now() + chrono::Duration::hours(4)),
            last_sync_count: Some(10),
            last_sync_success: Some(true),
        };

        let json = serde_json::to_string(&state).unwrap();
        let parsed: SyncState = serde_json::from_str(&json).unwrap();

        assert!(parsed.last_sync_at.is_some());
        assert!(parsed.next_sync_at.is_some());
        assert_eq!(parsed.last_sync_count, Some(10));
        assert_eq!(parsed.last_sync_success, Some(true));
    }
}

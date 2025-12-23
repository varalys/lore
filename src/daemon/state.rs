//! Daemon state management.
//!
//! Manages the daemon's runtime state including PID file, socket path,
//! and log file locations. Provides methods for checking if the daemon
//! is running and managing its lifecycle.

use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Manages daemon state including paths for PID file, socket, and logs.
///
/// The daemon uses files in `~/.lore/` to coordinate between the running
/// daemon process and CLI commands that interact with it.
pub struct DaemonState {
    /// Path to the PID file (`~/.lore/daemon.pid`).
    pub pid_file: PathBuf,
    /// Path to the Unix socket (`~/.lore/daemon.sock`).
    pub socket_path: PathBuf,
    /// Path to the log file (`~/.lore/daemon.log`).
    pub log_file: PathBuf,
}

impl DaemonState {
    /// Creates a new DaemonState with default paths in `~/.lore/`.
    ///
    /// Creates the `~/.lore/` directory if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined or
    /// if the `.lore` directory cannot be created.
    pub fn new() -> Result<Self> {
        let lore_dir = dirs::home_dir()
            .context("Could not find home directory")?
            .join(".lore");

        fs::create_dir_all(&lore_dir)
            .context("Failed to create ~/.lore directory")?;

        Ok(Self {
            pid_file: lore_dir.join("daemon.pid"),
            socket_path: lore_dir.join("daemon.sock"),
            log_file: lore_dir.join("daemon.log"),
        })
    }

    /// Checks if the daemon is currently running.
    ///
    /// Returns true if a PID file exists and the process with that PID
    /// is still alive. Returns false if no PID file exists, the PID file
    /// cannot be read, or the process is no longer running.
    pub fn is_running(&self) -> bool {
        match self.get_pid() {
            Some(pid) => Self::process_exists(pid),
            None => false,
        }
    }

    /// Gets the PID of the running daemon, if available.
    ///
    /// Returns `None` if the PID file does not exist or cannot be parsed.
    pub fn get_pid(&self) -> Option<u32> {
        if !self.pid_file.exists() {
            return None;
        }

        let mut file = fs::File::open(&self.pid_file).ok()?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).ok()?;

        contents.trim().parse().ok()
    }

    /// Writes the current process ID to the PID file.
    ///
    /// # Errors
    ///
    /// Returns an error if the PID file cannot be created or written to.
    pub fn write_pid(&self, pid: u32) -> Result<()> {
        let mut file = fs::File::create(&self.pid_file)
            .context("Failed to create PID file")?;
        write!(file, "{pid}")
            .context("Failed to write PID")?;
        Ok(())
    }

    /// Removes the PID file.
    ///
    /// Does not return an error if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be removed.
    pub fn remove_pid(&self) -> Result<()> {
        if self.pid_file.exists() {
            fs::remove_file(&self.pid_file)
                .context("Failed to remove PID file")?;
        }
        Ok(())
    }

    /// Removes the Unix socket file.
    ///
    /// Does not return an error if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be removed.
    pub fn remove_socket(&self) -> Result<()> {
        if self.socket_path.exists() {
            fs::remove_file(&self.socket_path)
                .context("Failed to remove socket file")?;
        }
        Ok(())
    }

    /// Cleans up all daemon state files (PID file and socket).
    ///
    /// Called during graceful shutdown.
    pub fn cleanup(&self) -> Result<()> {
        self.remove_pid()?;
        self.remove_socket()?;
        Ok(())
    }

    /// Checks if a process with the given PID exists.
    ///
    /// Uses the `kill(pid, 0)` system call which checks for process
    /// existence without sending a signal.
    fn process_exists(pid: u32) -> bool {
        // On Unix, sending signal 0 checks if process exists
        #[cfg(unix)]
        {
            // SAFETY: kill(pid, 0) is a safe system call that only checks
            // if a process exists without sending any signal.
            unsafe {
                libc::kill(pid as libc::pid_t, 0) == 0
            }
        }

        #[cfg(not(unix))]
        {
            // On Windows, we would need a different approach
            // For now, assume process exists if PID file exists
            let _ = pid;
            true
        }
    }
}

/// Statistics about the daemon's operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonStats {
    /// Number of session files currently being watched.
    pub files_watched: usize,
    /// Total number of sessions imported since daemon started.
    pub sessions_imported: u64,
    /// Total number of messages imported since daemon started.
    pub messages_imported: u64,
    /// Timestamp when the daemon started.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Number of errors encountered.
    pub errors: u64,
}

impl Default for DaemonStats {
    fn default() -> Self {
        Self {
            files_watched: 0,
            sessions_imported: 0,
            messages_imported: 0,
            started_at: chrono::Utc::now(),
            errors: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Creates a DaemonState with paths in a temporary directory.
    fn create_test_state() -> (DaemonState, tempfile::TempDir) {
        let dir = tempdir().expect("Failed to create temp directory");
        let state = DaemonState {
            pid_file: dir.path().join("daemon.pid"),
            socket_path: dir.path().join("daemon.sock"),
            log_file: dir.path().join("daemon.log"),
        };
        (state, dir)
    }

    #[test]
    fn test_is_running_no_pid_file() {
        let (state, _dir) = create_test_state();
        assert!(!state.is_running(), "Should not be running without PID file");
    }

    #[test]
    fn test_get_pid_no_file() {
        let (state, _dir) = create_test_state();
        assert!(state.get_pid().is_none(), "Should return None without PID file");
    }

    #[test]
    fn test_write_and_get_pid() {
        let (state, _dir) = create_test_state();

        state.write_pid(12345).expect("Failed to write PID");

        let pid = state.get_pid();
        assert_eq!(pid, Some(12345), "PID should match written value");
    }

    #[test]
    fn test_remove_pid() {
        let (state, _dir) = create_test_state();

        state.write_pid(12345).expect("Failed to write PID");
        assert!(state.pid_file.exists(), "PID file should exist after write");

        state.remove_pid().expect("Failed to remove PID");
        assert!(!state.pid_file.exists(), "PID file should not exist after remove");
    }

    #[test]
    fn test_remove_pid_nonexistent() {
        let (state, _dir) = create_test_state();

        // Should not error when file doesn't exist
        state.remove_pid().expect("Should not error on nonexistent file");
    }

    #[test]
    fn test_remove_socket() {
        let (state, _dir) = create_test_state();

        // Create a fake socket file
        fs::write(&state.socket_path, "").expect("Failed to create file");
        assert!(state.socket_path.exists(), "Socket file should exist");

        state.remove_socket().expect("Failed to remove socket");
        assert!(!state.socket_path.exists(), "Socket file should not exist after remove");
    }

    #[test]
    fn test_cleanup() {
        let (state, _dir) = create_test_state();

        state.write_pid(12345).expect("Failed to write PID");
        fs::write(&state.socket_path, "").expect("Failed to create socket");

        state.cleanup().expect("Failed to cleanup");

        assert!(!state.pid_file.exists(), "PID file should be cleaned up");
        assert!(!state.socket_path.exists(), "Socket file should be cleaned up");
    }

    #[test]
    fn test_daemon_stats_default() {
        let stats = DaemonStats::default();

        assert_eq!(stats.files_watched, 0);
        assert_eq!(stats.sessions_imported, 0);
        assert_eq!(stats.messages_imported, 0);
        assert_eq!(stats.errors, 0);
    }

    #[test]
    fn test_is_running_with_invalid_pid() {
        let (state, _dir) = create_test_state();

        // Write an invalid PID (likely not a real process)
        state.write_pid(999999999).expect("Failed to write PID");

        // This should return false since the process likely doesn't exist
        // (though it could theoretically be a valid PID on some systems)
        let running = state.is_running();
        // We can't assert definitively since the behavior depends on the system
        // Just verify it doesn't panic
        let _ = running;
    }

    #[test]
    fn test_get_pid_invalid_content() {
        let (state, _dir) = create_test_state();

        // Write invalid content to PID file
        fs::write(&state.pid_file, "not_a_number").expect("Failed to write");

        assert!(state.get_pid().is_none(), "Should return None for invalid PID");
    }
}

//! Background daemon for automatic session capture.
//!
//! The daemon watches for Claude Code session files and automatically
//! imports them into the Lore database. It provides:
//!
//! - File watching for `~/.claude/projects/` directory
//! - Incremental parsing of session files
//! - Unix socket IPC for CLI communication
//! - Graceful shutdown handling
//!
//! # Architecture
//!
//! The daemon consists of three main components:
//!
//! - **Watcher**: Monitors the file system for new/modified session files
//! - **Server**: Handles IPC commands from CLI (status, stop, stats)
//! - **State**: Manages PID file, socket path, and runtime state
//!
//! # Usage
//!
//! The daemon is typically started via `lore daemon start` and can be
//! stopped via `lore daemon stop`. Use `lore daemon status` to check
//! if the daemon is running.

pub mod server;
pub mod state;
pub mod watcher;

use anyhow::Result;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{oneshot, RwLock};
use tracing_appender::non_blocking::WorkerGuard;

use crate::config::Config;

pub use server::{send_command_sync, DaemonCommand, DaemonResponse};
pub use state::{DaemonState, DaemonStats};
pub use watcher::SessionWatcher;

/// Runs the daemon in the foreground.
///
/// This is the main entry point for the daemon. It:
/// 1. Checks if another instance is already running
/// 2. Sets up logging to a file
/// 3. Writes the PID file
/// 4. Starts the file watcher and IPC server
/// 5. Waits for shutdown signal (SIGTERM/SIGINT or stop command)
/// 6. Cleans up state files on exit
///
/// # Errors
///
/// Returns an error if:
/// - Another daemon instance is already running
/// - The database cannot be opened
/// - The watcher or server fails to start
pub async fn run_daemon() -> Result<()> {
    let state = DaemonState::new()?;

    // Check if already running
    if state.is_running() {
        anyhow::bail!(
            "Daemon is already running (PID {})",
            state.get_pid().unwrap_or(0)
        );
    }

    // Check if lore has been initialized
    let config_path = Config::config_path()?;
    if !config_path.exists() {
        anyhow::bail!(
            "Lore has not been initialized.\n\n\
            Run 'lore init' first to:\n  \
            - Select which AI tools to watch\n  \
            - Configure your machine identity\n  \
            - Import existing sessions\n\n\
            Then start the daemon with 'lore daemon start' or let init do it for you."
        );
    }

    // Set up file logging
    let _guard = setup_logging(&state)?;

    tracing::info!("Starting Lore daemon...");

    // Write PID file
    let pid = std::process::id();
    state.write_pid(pid)?;
    tracing::info!("Daemon started with PID {}", pid);

    // Create shared stats
    let stats = Arc::new(RwLock::new(DaemonStats::default()));

    // Create shutdown channels
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Start the IPC server
    let server_stats = stats.clone();
    let socket_path = state.socket_path.clone();
    let server_broadcast_rx = broadcast_tx.subscribe();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server::run_server(
            &socket_path,
            server_stats,
            Some(stop_tx),
            server_broadcast_rx,
        )
        .await
        {
            tracing::error!("IPC server error: {}", e);
        }
    });

    // Start the file watcher
    let mut watcher = SessionWatcher::new()?;
    let watcher_stats = stats.clone();
    let watcher_broadcast_rx = broadcast_tx.subscribe();
    let watcher_handle = tokio::spawn(async move {
        if let Err(e) = watcher.watch(watcher_stats, watcher_broadcast_rx).await {
            tracing::error!("Watcher error: {}", e);
        }
    });

    // Wait for shutdown signal
    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down...");
        }
        _ = stop_rx => {
            tracing::info!("Received stop command, shutting down...");
        }
    }

    // Signal all components to shut down
    let _ = broadcast_tx.send(());

    // Give components time to clean up
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Abort handles if they haven't finished
    server_handle.abort();
    watcher_handle.abort();

    // Clean up state files
    state.cleanup()?;

    tracing::info!("Daemon stopped");

    Ok(())
}

/// Sets up file logging for the daemon.
///
/// Configures tracing to write logs to `~/.lore/daemon.log`.
/// Returns a guard that must be kept alive for the duration of the daemon.
/// If a global subscriber is already set (e.g., from main.rs when running
/// in foreground mode), this will log to the existing subscriber.
fn setup_logging(state: &DaemonState) -> Result<WorkerGuard> {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let file_appender = tracing_appender::rolling::never(
        state.log_file.parent().unwrap_or(std::path::Path::new(".")),
        state.log_file.file_name().unwrap_or_default(),
    );
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);

    // Use try_init to avoid panic if a subscriber is already set
    // (which happens when running in foreground from CLI)
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lore=info".into()),
        )
        .with(file_layer)
        .try_init();

    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_state_paths() {
        // Just verify DaemonState can be created
        let state = DaemonState::new();
        assert!(state.is_ok(), "DaemonState creation should succeed");

        let state = state.unwrap();
        assert!(
            state.pid_file.to_string_lossy().contains("daemon.pid"),
            "PID file path should contain daemon.pid"
        );
        assert!(
            state.socket_path.to_string_lossy().contains("daemon.sock"),
            "Socket path should contain daemon.sock"
        );
        assert!(
            state.log_file.to_string_lossy().contains("daemon.log"),
            "Log file path should contain daemon.log"
        );
    }
}

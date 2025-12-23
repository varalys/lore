//! Unix socket IPC server for daemon communication.
//!
//! Provides a simple request/response protocol over Unix domain sockets
//! for communicating with the running daemon. Supports commands like
//! status, stop, and stats.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{oneshot, RwLock};

use super::state::DaemonStats;

/// Commands that can be sent to the daemon via IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum DaemonCommand {
    /// Request the daemon's current status.
    Status,
    /// Request the daemon to shut down gracefully.
    Stop,
    /// Request runtime statistics from the daemon.
    Stats,
    /// Ping to check if daemon is responsive.
    Ping,
}

/// Responses from the daemon to IPC commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    /// Status response indicating daemon is running.
    Status {
        running: bool,
        pid: u32,
        uptime_seconds: u64,
    },
    /// Acknowledgment that stop command was received.
    Stopping,
    /// Runtime statistics.
    Stats(DaemonStats),
    /// Ping response.
    Pong,
    /// Error response.
    Error { message: String },
}

/// Runs the IPC server on the given Unix socket path.
///
/// The server listens for incoming connections and processes commands
/// until a shutdown signal is received or the Stop command is sent.
///
/// # Arguments
///
/// * `socket_path` - Path for the Unix domain socket
/// * `stats` - Shared statistics that can be read by clients
/// * `shutdown_tx` - Sender to signal daemon shutdown when Stop is received
/// * `mut shutdown_rx` - Receiver that signals when to stop the server
///
/// # Errors
///
/// Returns an error if the socket cannot be created or bound.
pub async fn run_server(
    socket_path: &Path,
    stats: Arc<RwLock<DaemonStats>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    // Remove existing socket file if present
    if socket_path.exists() {
        std::fs::remove_file(socket_path)
            .context("Failed to remove existing socket file")?;
    }

    let listener = UnixListener::bind(socket_path)
        .context("Failed to bind Unix socket")?;

    tracing::info!("IPC server listening on {:?}", socket_path);

    // Wrap shutdown_tx in Arc<Mutex> so it can be moved into the handler
    let shutdown_tx = Arc::new(std::sync::Mutex::new(shutdown_tx));

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _addr)) => {
                        let stats_clone = stats.clone();
                        let shutdown_tx_clone = shutdown_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, stats_clone, shutdown_tx_clone).await {
                                tracing::warn!("Error handling IPC connection: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to accept connection: {}", e);
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("IPC server shutting down");
                break;
            }
        }
    }

    Ok(())
}

/// Handles a single client connection.
async fn handle_connection(
    stream: UnixStream,
    stats: Arc<RwLock<DaemonStats>>,
    shutdown_tx: Arc<std::sync::Mutex<Option<oneshot::Sender<()>>>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Read a single line (one command per connection)
    reader.read_line(&mut line).await
        .context("Failed to read from socket")?;

    let command: DaemonCommand = serde_json::from_str(line.trim())
        .context("Failed to parse command")?;

    tracing::debug!("Received IPC command: {:?}", command);

    let response = match command {
        DaemonCommand::Status => {
            let stats_guard = stats.read().await;
            let uptime = chrono::Utc::now()
                .signed_duration_since(stats_guard.started_at)
                .num_seconds() as u64;
            DaemonResponse::Status {
                running: true,
                pid: std::process::id(),
                uptime_seconds: uptime,
            }
        }
        DaemonCommand::Stop => {
            // Signal the daemon to shut down
            let mut guard = shutdown_tx.lock().unwrap();
            if let Some(tx) = guard.take() {
                let _ = tx.send(());
            }
            DaemonResponse::Stopping
        }
        DaemonCommand::Stats => {
            let stats_guard = stats.read().await;
            DaemonResponse::Stats(stats_guard.clone())
        }
        DaemonCommand::Ping => DaemonResponse::Pong,
    };

    let response_json = serde_json::to_string(&response)
        .context("Failed to serialize response")?;

    writer.write_all(response_json.as_bytes()).await
        .context("Failed to write response")?;
    writer.write_all(b"\n").await
        .context("Failed to write newline")?;
    writer.flush().await
        .context("Failed to flush writer")?;

    Ok(())
}

/// Sends a command to the daemon and returns the response.
///
/// Connects to the Unix socket, sends the command, and reads the response.
///
/// # Arguments
///
/// * `socket_path` - Path to the daemon's Unix socket
/// * `command` - The command to send
///
/// # Errors
///
/// Returns an error if the connection fails, the command cannot be sent,
/// or the response cannot be read or parsed.
pub async fn send_command(socket_path: &Path, command: DaemonCommand) -> Result<DaemonResponse> {
    let stream = UnixStream::connect(socket_path).await
        .context("Failed to connect to daemon socket")?;

    let (reader, mut writer) = stream.into_split();

    // Send command
    let command_json = serde_json::to_string(&command)
        .context("Failed to serialize command")?;
    writer.write_all(command_json.as_bytes()).await
        .context("Failed to write command")?;
    writer.write_all(b"\n").await
        .context("Failed to write newline")?;
    writer.flush().await
        .context("Failed to flush")?;

    // Read response
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await
        .context("Failed to read response")?;

    let response: DaemonResponse = serde_json::from_str(line.trim())
        .context("Failed to parse response")?;

    Ok(response)
}

/// Synchronous wrapper for sending a command to the daemon.
///
/// Creates a temporary tokio runtime to send the command.
/// Use this from non-async contexts like CLI commands.
pub fn send_command_sync(socket_path: &Path, command: DaemonCommand) -> Result<DaemonResponse> {
    let rt = tokio::runtime::Runtime::new()
        .context("Failed to create tokio runtime")?;
    rt.block_on(send_command(socket_path, command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_daemon_command_serialization() {
        let commands = vec![
            DaemonCommand::Status,
            DaemonCommand::Stop,
            DaemonCommand::Stats,
            DaemonCommand::Ping,
        ];

        for cmd in commands {
            let json = serde_json::to_string(&cmd).expect("Failed to serialize");
            let parsed: DaemonCommand = serde_json::from_str(&json).expect("Failed to parse");
            // Just verify round-trip works (can't compare Debug output reliably)
            let _ = parsed;
        }
    }

    #[test]
    fn test_daemon_response_status_serialization() {
        let response = DaemonResponse::Status {
            running: true,
            pid: 12345,
            uptime_seconds: 3600,
        };

        let json = serde_json::to_string(&response).expect("Failed to serialize");
        assert!(json.contains("\"type\":\"status\""));
        assert!(json.contains("\"running\":true"));
        assert!(json.contains("\"pid\":12345"));

        let parsed: DaemonResponse = serde_json::from_str(&json).expect("Failed to parse");
        match parsed {
            DaemonResponse::Status { running, pid, uptime_seconds } => {
                assert!(running);
                assert_eq!(pid, 12345);
                assert_eq!(uptime_seconds, 3600);
            }
            _ => panic!("Expected Status response"),
        }
    }

    #[test]
    fn test_daemon_response_stopping_serialization() {
        let response = DaemonResponse::Stopping;
        let json = serde_json::to_string(&response).expect("Failed to serialize");
        assert!(json.contains("\"type\":\"stopping\""));
    }

    #[test]
    fn test_daemon_response_stats_serialization() {
        let stats = DaemonStats::default();
        let response = DaemonResponse::Stats(stats);

        let json = serde_json::to_string(&response).expect("Failed to serialize");
        assert!(json.contains("\"type\":\"stats\""));
        assert!(json.contains("\"files_watched\""));
    }

    #[test]
    fn test_daemon_response_error_serialization() {
        let response = DaemonResponse::Error {
            message: "Something went wrong".to_string(),
        };

        let json = serde_json::to_string(&response).expect("Failed to serialize");
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("Something went wrong"));
    }

    #[tokio::test]
    async fn test_server_client_communication() {
        let dir = tempdir().expect("Failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let stats = Arc::new(RwLock::new(DaemonStats::default()));
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        let (broadcast_tx, broadcast_rx) = tokio::sync::broadcast::channel(1);

        // Start server in background
        let socket_path_clone = socket_path.clone();
        let stats_clone = stats.clone();
        let server_handle = tokio::spawn(async move {
            run_server(&socket_path_clone, stats_clone, Some(shutdown_tx), broadcast_rx).await
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Send ping command
        let response = send_command(&socket_path, DaemonCommand::Ping).await
            .expect("Failed to send command");

        match response {
            DaemonResponse::Pong => {}
            _ => panic!("Expected Pong response"),
        }

        // Send status command
        let response = send_command(&socket_path, DaemonCommand::Status).await
            .expect("Failed to send command");

        match response {
            DaemonResponse::Status { running, .. } => {
                assert!(running);
            }
            _ => panic!("Expected Status response"),
        }

        // Send stop command
        let response = send_command(&socket_path, DaemonCommand::Stop).await
            .expect("Failed to send command");

        match response {
            DaemonResponse::Stopping => {}
            _ => panic!("Expected Stopping response"),
        }

        // Signal broadcast shutdown and wait for server to stop
        let _ = broadcast_tx.send(());
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            server_handle
        ).await;
    }
}

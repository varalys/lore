//! Daemon management commands.
//!
//! Provides CLI commands for starting, stopping, and monitoring the
//! background daemon that watches for Claude Code session files.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::process::Command;

use crate::daemon::{
    send_command_sync, DaemonCommand, DaemonResponse, DaemonState,
};

/// Daemon management subcommands.
#[derive(Subcommand)]
pub enum DaemonSubcommand {
    /// Start the background daemon.
    Start {
        /// Run in foreground (don't daemonize).
        #[arg(long)]
        foreground: bool,
    },

    /// Stop the running daemon.
    Stop,

    /// Show daemon status.
    Status,

    /// Show daemon logs.
    Logs {
        /// Number of lines to show.
        #[arg(short = 'n', long, default_value = "20")]
        lines: usize,

        /// Follow log output (like tail -f).
        #[arg(short, long)]
        follow: bool,
    },
}

/// Arguments for the daemon command.
#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: DaemonSubcommand,
}

/// Executes the daemon command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        DaemonSubcommand::Start { foreground } => run_start(foreground),
        DaemonSubcommand::Stop => run_stop(),
        DaemonSubcommand::Status => run_status(),
        DaemonSubcommand::Logs { lines, follow } => run_logs(lines, follow),
    }
}

/// Starts the daemon.
fn run_start(foreground: bool) -> Result<()> {
    let state = DaemonState::new()?;

    // Check if already running
    if state.is_running() {
        let pid = state.get_pid().unwrap_or(0);
        println!(
            "{} Daemon is already running (PID {})",
            "Warning:".yellow(),
            pid
        );
        return Ok(());
    }

    if foreground {
        println!("{}", "Starting daemon in foreground...".green());
        println!("{}", "Press Ctrl+C to stop".dimmed());
        println!();

        // Run the daemon in the current process
        let rt = tokio::runtime::Runtime::new()
            .context("Failed to create tokio runtime")?;

        rt.block_on(crate::daemon::run_daemon())?;
    } else {
        // Start the daemon as a background process
        println!("{}", "Starting daemon in background...".green());

        let current_exe = std::env::current_exe()
            .context("Failed to get current executable path")?;

        // Spawn the daemon process with foreground flag
        // The child process will handle daemonization
        let child = Command::new(&current_exe)
            .arg("daemon")
            .arg("start")
            .arg("--foreground")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to spawn daemon process")?;

        println!(
            "{} Daemon started with PID {}",
            "Success:".green(),
            child.id()
        );
        println!(
            "{}",
            format!("Logs available at: {:?}", state.log_file).dimmed()
        );
    }

    Ok(())
}

/// Stops the running daemon.
fn run_stop() -> Result<()> {
    let state = DaemonState::new()?;

    if !state.is_running() {
        println!("{}", "Daemon is not running".yellow());
        return Ok(());
    }

    let pid = state.get_pid().unwrap_or(0);
    println!("Stopping daemon (PID {pid})...");

    // Try to send stop command via socket first
    match send_command_sync(&state.socket_path, DaemonCommand::Stop) {
        Ok(DaemonResponse::Stopping) => {
            println!("{}", "Stop command sent".green());

            // Wait for daemon to stop
            for i in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if !state.is_running() {
                    println!("{}", "Daemon stopped".green());
                    return Ok(());
                }
                if i == 10 {
                    println!("{}", "Waiting for daemon to stop...".dimmed());
                }
            }

            // If still running after 3 seconds, try SIGTERM
            println!("{}", "Daemon did not stop gracefully, sending SIGTERM...".yellow());
            kill_process(pid)?;
        }
        Ok(resp) => {
            println!("Unexpected response: {resp:?}");
            // Fall back to SIGTERM
            kill_process(pid)?;
        }
        Err(e) => {
            tracing::debug!("Failed to send stop command: {}", e);
            // Socket might not be available, try SIGTERM
            println!("{}", "Socket not available, sending SIGTERM...".yellow());
            kill_process(pid)?;
        }
    }

    // Wait a bit and check if stopped
    std::thread::sleep(std::time::Duration::from_secs(1));
    if !state.is_running() {
        // Clean up any leftover files
        let _ = state.cleanup();
        println!("{}", "Daemon stopped".green());
    } else {
        println!("{}", "Warning: Daemon may still be running".yellow());
    }

    Ok(())
}

/// Sends SIGTERM to a process.
fn kill_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }

    #[cfg(not(unix))]
    {
        // On Windows, we would need a different approach
        anyhow::bail!("Killing processes not supported on this platform");
    }

    Ok(())
}

/// Shows the daemon status.
fn run_status() -> Result<()> {
    let state = DaemonState::new()?;

    if !state.is_running() {
        println!("{}", "Daemon is not running".yellow());
        return Ok(());
    }

    let pid = state.get_pid().unwrap_or(0);

    // Try to get detailed status via socket
    match send_command_sync(&state.socket_path, DaemonCommand::Status) {
        Ok(DaemonResponse::Status { running: _, pid: actual_pid, uptime_seconds }) => {
            println!("{}", "Daemon Status".green().bold());
            println!();
            println!("  {} {}", "Status:".dimmed(), "running".green());
            println!("  {} {}", "PID:".dimmed(), actual_pid);
            println!("  {} {}", "Uptime:".dimmed(), format_duration(uptime_seconds));

            // Try to get stats
            if let Ok(DaemonResponse::Stats(stats)) = send_command_sync(&state.socket_path, DaemonCommand::Stats) {
                println!();
                println!("{}", "Statistics".green().bold());
                println!();
                println!("  {} {}", "Files watched:".dimmed(), stats.files_watched);
                println!("  {} {}", "Sessions imported:".dimmed(), stats.sessions_imported);
                println!("  {} {}", "Messages imported:".dimmed(), stats.messages_imported);
                if stats.errors > 0 {
                    println!("  {} {}", "Errors:".dimmed(), stats.errors.to_string().red());
                }
            }
        }
        Ok(_) => {
            // Unexpected response, just show basic info
            println!("{}", "Daemon Status".green().bold());
            println!();
            println!("  {} {}", "Status:".dimmed(), "running".green());
            println!("  {} {}", "PID:".dimmed(), pid);
        }
        Err(e) => {
            // Can't connect to socket, but PID file exists
            tracing::debug!("Failed to get status: {}", e);
            println!("{}", "Daemon Status".green().bold());
            println!();
            println!("  {} {} {}", "Status:".dimmed(), "running".green(), "(socket unavailable)".dimmed());
            println!("  {} {}", "PID:".dimmed(), pid);
        }
    }

    Ok(())
}

/// Formats a duration in seconds as a human-readable string.
fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        let mins = seconds / 60;
        let secs = seconds % 60;
        format!("{mins}m {secs}s")
    } else if seconds < 86400 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        format!("{hours}h {mins}m")
    } else {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        format!("{days}d {hours}h")
    }
}

/// Shows daemon logs.
fn run_logs(lines: usize, follow: bool) -> Result<()> {
    let state = DaemonState::new()?;

    if !state.log_file.exists() {
        println!("{}", "No log file found".yellow());
        println!(
            "{}",
            format!("Expected at: {:?}", state.log_file).dimmed()
        );
        return Ok(());
    }

    if follow {
        // Follow mode - continuously read new lines
        println!("{}", format!("Following {:?}...", state.log_file).dimmed());
        println!("{}", "Press Ctrl+C to stop".dimmed());
        println!();

        let file = File::open(&state.log_file)
            .context("Failed to open log file")?;
        let mut reader = BufReader::new(file);

        // Seek to end
        reader.seek(SeekFrom::End(0))?;

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // No new data, sleep briefly
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Ok(_) => {
                    print!("{line}");
                }
                Err(e) => {
                    tracing::debug!("Error reading log: {}", e);
                    break;
                }
            }
        }
    } else {
        // Show last N lines
        let file = File::open(&state.log_file)
            .context("Failed to open log file")?;
        let reader = BufReader::new(file);

        let all_lines: Vec<String> = reader
            .lines()
            .map_while(Result::ok)
            .collect();

        let start = if all_lines.len() > lines {
            all_lines.len() - lines
        } else {
            0
        };

        for line in &all_lines[start..] {
            println!("{line}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60), "1m 0s");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3599), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600), "1h 0m");
        assert_eq!(format_duration(7200), "2h 0m");
        assert_eq!(format_duration(86399), "23h 59m");
    }

    #[test]
    fn test_format_duration_days() {
        assert_eq!(format_duration(86400), "1d 0h");
        assert_eq!(format_duration(172800), "2d 0h");
        assert_eq!(format_duration(90000), "1d 1h");
    }
}

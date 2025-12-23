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
    /// Start the background daemon
    #[command(long_about = "Starts the background daemon that watches for new AI\n\
        coding sessions and automatically imports them into the\n\
        database. The daemon runs in the background by default.")]
    Start {
        /// Run in foreground instead of daemonizing
        #[arg(long)]
        #[arg(long_help = "Run the daemon in the foreground instead of as a\n\
            background process. Useful for debugging or when\n\
            running under a process supervisor.")]
        foreground: bool,
    },

    /// Stop the running daemon
    #[command(long_about = "Sends a stop signal to the running daemon. The daemon\n\
        will finish any pending operations before shutting down.")]
    Stop,

    /// Show daemon status and statistics
    #[command(long_about = "Shows whether the daemon is running, its PID, uptime,\n\
        and statistics about watched files and imported sessions.")]
    Status,

    /// Show daemon logs
    #[command(long_about = "Displays recent log output from the daemon. Use -f to\n\
        follow the log in real-time (like 'tail -f').")]
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "20", value_name = "N")]
        lines: usize,

        /// Follow log output in real-time (like tail -f)
        #[arg(short, long)]
        #[arg(long_help = "Continuously display new log lines as they are written.\n\
            Press Ctrl+C to stop following.")]
        follow: bool,
    },

    /// Install the daemon as a system service
    #[command(long_about = "Installs the Lore daemon as a system service that starts\n\
        automatically on user login.\n\n\
        On macOS: Creates a launchd plist in ~/Library/LaunchAgents/\n\
        On Linux: Creates a systemd user unit in ~/.config/systemd/user/")]
    Install,

    /// Uninstall the daemon system service
    #[command(long_about = "Removes the Lore daemon system service and stops it if running.\n\n\
        On macOS: Unloads and removes the launchd plist\n\
        On Linux: Disables and removes the systemd user unit")]
    Uninstall,
}

/// Arguments for the daemon command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore daemon start             Start in background\n    \
    lore daemon start --foreground Run in foreground\n    \
    lore daemon stop              Stop the daemon\n    \
    lore daemon status            Check if running\n    \
    lore daemon logs              Show recent logs\n    \
    lore daemon logs -f           Follow logs in real-time\n    \
    lore daemon install           Install as system service\n    \
    lore daemon uninstall         Remove system service")]
pub struct Args {
    /// Daemon subcommand to run
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
        DaemonSubcommand::Install => run_install(),
        DaemonSubcommand::Uninstall => run_uninstall(),
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
        let _ = pid; // Suppress unused variable warning
        anyhow::bail!(
            "Stopping the daemon by process signal is not supported on this platform. \
             Try running 'lore daemon stop' again or manually terminate the process."
        );
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

// Service installation constants
const LAUNCHD_LABEL: &str = "com.lore.daemon";
#[cfg(any(target_os = "linux", test))]
const SYSTEMD_SERVICE_NAME: &str = "lore";

/// Generates a macOS launchd plist for the daemon service.
///
/// The plist configures launchd to:
/// - Start the daemon on user login (RunAtLoad)
/// - Keep the daemon running (KeepAlive)
/// - Restart on failure
/// - Direct logs to the lore logs directory
fn generate_launchd_plist(lore_exe: &std::path::Path, logs_dir: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{logs_dir}/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>{logs_dir}/daemon.err</string>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        exe = lore_exe.display(),
        logs_dir = logs_dir.display(),
    )
}

/// Generates a Linux systemd user unit for the daemon service.
///
/// The unit configures systemd to:
/// - Start after the default target
/// - Run as a simple service
/// - Restart on failure with a 5-second delay
/// - Be enabled at user login
#[cfg(any(target_os = "linux", test))]
fn generate_systemd_unit(lore_exe: &std::path::Path) -> String {
    format!(
        r#"[Unit]
Description=Lore AI session capture daemon
After=default.target

[Service]
Type=simple
ExecStart={exe} daemon start --foreground
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"#,
        exe = lore_exe.display(),
    )
}

/// Gets the path to the launchd plist file.
fn get_launchd_plist_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join("Library/LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist")))
}

/// Gets the path to the systemd user unit file.
#[cfg(any(target_os = "linux", test))]
fn get_systemd_unit_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".config/systemd/user").join(format!("{SYSTEMD_SERVICE_NAME}.service")))
}

/// Gets the logs directory path, creating it if necessary.
fn ensure_logs_dir() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let logs_dir = home.join(".lore/logs");
    std::fs::create_dir_all(&logs_dir).context("Failed to create logs directory")?;
    Ok(logs_dir)
}

/// Installs the daemon as a system service.
fn run_install() -> Result<()> {
    let lore_exe = std::env::current_exe().context("Failed to get current executable path")?;

    #[cfg(target_os = "macos")]
    {
        install_launchd_service(&lore_exe)
    }

    #[cfg(target_os = "linux")]
    {
        install_systemd_service(&lore_exe)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = lore_exe; // Suppress unused variable warning
        anyhow::bail!(
            "Service installation is not supported on this platform.\n\
             Supported platforms: macOS (launchd), Linux (systemd)"
        );
    }
}

/// Installs the daemon as a macOS launchd service.
#[cfg(target_os = "macos")]
fn install_launchd_service(lore_exe: &std::path::Path) -> Result<()> {
    let plist_path = get_launchd_plist_path()?;
    let logs_dir = ensure_logs_dir()?;

    // Check if already installed
    if plist_path.exists() {
        println!(
            "{} Service is already installed at {}",
            "Warning:".yellow(),
            plist_path.display()
        );
        println!(
            "{}",
            "Run 'lore daemon uninstall' first to reinstall.".dimmed()
        );
        return Ok(());
    }

    // Ensure LaunchAgents directory exists
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create LaunchAgents directory")?;
    }

    // Generate and write the plist
    let plist_content = generate_launchd_plist(lore_exe, &logs_dir);
    std::fs::write(&plist_path, &plist_content).context("Failed to write plist file")?;

    println!("{}", "Installing Lore daemon service...".green());

    // Load the service immediately
    let output = Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&plist_path)
        .output()
        .context("Failed to run launchctl load")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Clean up the plist if load failed
        let _ = std::fs::remove_file(&plist_path);
        anyhow::bail!("Failed to load service: {}", stderr.trim());
    }

    println!("{}", "Service installed successfully!".green());
    println!();
    println!("  {} {}", "Plist:".dimmed(), plist_path.display());
    println!("  {} {}", "Logs:".dimmed(), logs_dir.join("daemon.log").display());
    println!();
    println!(
        "{}",
        "The daemon will start automatically on login.".dimmed()
    );
    println!(
        "{}",
        "Use 'lore daemon status' to check if it's running.".dimmed()
    );

    Ok(())
}

/// Installs the daemon as a Linux systemd user service.
#[cfg(target_os = "linux")]
fn install_systemd_service(lore_exe: &std::path::Path) -> Result<()> {
    let unit_path = get_systemd_unit_path()?;

    // Check if already installed
    if unit_path.exists() {
        println!(
            "{} Service is already installed at {}",
            "Warning:".yellow(),
            unit_path.display()
        );
        println!(
            "{}",
            "Run 'lore daemon uninstall' first to reinstall.".dimmed()
        );
        return Ok(());
    }

    // Ensure systemd user directory exists
    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create systemd user directory")?;
    }

    // Generate and write the unit file
    let unit_content = generate_systemd_unit(lore_exe);
    std::fs::write(&unit_path, &unit_content).context("Failed to write unit file")?;

    println!("{}", "Installing Lore daemon service...".green());

    // Reload systemd user daemon to pick up the new unit
    let reload = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output()
        .context("Failed to run systemctl daemon-reload")?;

    if !reload.status.success() {
        let stderr = String::from_utf8_lossy(&reload.stderr);
        let _ = std::fs::remove_file(&unit_path);
        anyhow::bail!("Failed to reload systemd: {}", stderr.trim());
    }

    // Enable and start the service
    let enable = Command::new("systemctl")
        .args(["--user", "enable", "--now", SYSTEMD_SERVICE_NAME])
        .output()
        .context("Failed to run systemctl enable")?;

    if !enable.status.success() {
        let stderr = String::from_utf8_lossy(&enable.stderr);
        let _ = std::fs::remove_file(&unit_path);
        anyhow::bail!("Failed to enable service: {}", stderr.trim());
    }

    println!("{}", "Service installed successfully!".green());
    println!();
    println!("  {} {}", "Unit file:".dimmed(), unit_path.display());
    println!();
    println!(
        "{}",
        "The daemon will start automatically on login.".dimmed()
    );
    println!(
        "{}",
        "Use 'lore daemon status' to check if it's running.".dimmed()
    );

    Ok(())
}

/// Uninstalls the daemon system service.
fn run_uninstall() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        uninstall_launchd_service()
    }

    #[cfg(target_os = "linux")]
    {
        uninstall_systemd_service()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!(
            "Service uninstallation is not supported on this platform.\n\
             Supported platforms: macOS (launchd), Linux (systemd)"
        );
    }
}

/// Uninstalls the macOS launchd service.
#[cfg(target_os = "macos")]
fn uninstall_launchd_service() -> Result<()> {
    let plist_path = get_launchd_plist_path()?;

    if !plist_path.exists() {
        println!("{}", "Service is not installed".yellow());
        return Ok(());
    }

    println!("{}", "Uninstalling Lore daemon service...".green());

    // Unload the service first (this also stops it)
    let output = Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(&plist_path)
        .output()
        .context("Failed to run launchctl unload")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Only warn, don't fail - the service might not be loaded
        if !stderr.contains("Could not find specified service") {
            println!(
                "{} launchctl unload: {}",
                "Warning:".yellow(),
                stderr.trim()
            );
        }
    }

    // Remove the plist file
    std::fs::remove_file(&plist_path).context("Failed to remove plist file")?;

    println!("{}", "Service uninstalled successfully!".green());
    println!();
    println!(
        "{}",
        "The daemon will no longer start automatically on login.".dimmed()
    );

    Ok(())
}

/// Uninstalls the Linux systemd user service.
#[cfg(target_os = "linux")]
fn uninstall_systemd_service() -> Result<()> {
    let unit_path = get_systemd_unit_path()?;

    if !unit_path.exists() {
        println!("{}", "Service is not installed".yellow());
        return Ok(());
    }

    println!("{}", "Uninstalling Lore daemon service...".green());

    // Stop and disable the service
    let disable = Command::new("systemctl")
        .args(["--user", "disable", "--now", SYSTEMD_SERVICE_NAME])
        .output()
        .context("Failed to run systemctl disable")?;

    if !disable.status.success() {
        let stderr = String::from_utf8_lossy(&disable.stderr);
        // Only warn, don't fail - the service might not be active
        if !stderr.contains("not loaded") {
            println!(
                "{} systemctl disable: {}",
                "Warning:".yellow(),
                stderr.trim()
            );
        }
    }

    // Remove the unit file
    std::fs::remove_file(&unit_path).context("Failed to remove unit file")?;

    // Reload systemd to forget about the unit
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    println!("{}", "Service uninstalled successfully!".green());
    println!();
    println!(
        "{}",
        "The daemon will no longer start automatically on login.".dimmed()
    );

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

    #[test]
    fn test_generate_launchd_plist_contains_required_fields() {
        let exe_path = std::path::Path::new("/usr/local/bin/lore");
        let logs_dir = std::path::Path::new("/Users/test/.lore/logs");

        let plist = generate_launchd_plist(exe_path, logs_dir);

        // Check XML declaration and DOCTYPE
        assert!(
            plist.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
            "Plist should have XML declaration"
        );
        assert!(
            plist.contains("<!DOCTYPE plist"),
            "Plist should have DOCTYPE"
        );

        // Check label
        assert!(
            plist.contains(&format!("<string>{LAUNCHD_LABEL}</string>")),
            "Plist should contain the correct label"
        );

        // Check executable path
        assert!(
            plist.contains("<string>/usr/local/bin/lore</string>"),
            "Plist should contain the executable path"
        );

        // Check program arguments
        assert!(
            plist.contains("<string>daemon</string>"),
            "Plist should have daemon argument"
        );
        assert!(
            plist.contains("<string>start</string>"),
            "Plist should have start argument"
        );
        assert!(
            plist.contains("<string>--foreground</string>"),
            "Plist should have --foreground argument"
        );

        // Check RunAtLoad
        assert!(
            plist.contains("<key>RunAtLoad</key>") && plist.contains("<true/>"),
            "Plist should have RunAtLoad set to true"
        );

        // Check KeepAlive
        assert!(
            plist.contains("<key>KeepAlive</key>"),
            "Plist should have KeepAlive key"
        );

        // Check log paths
        assert!(
            plist.contains("/Users/test/.lore/logs/daemon.log"),
            "Plist should contain stdout log path"
        );
        assert!(
            plist.contains("/Users/test/.lore/logs/daemon.err"),
            "Plist should contain stderr log path"
        );
    }

    #[test]
    fn test_generate_systemd_unit_contains_required_fields() {
        let exe_path = std::path::Path::new("/usr/local/bin/lore");

        let unit = generate_systemd_unit(exe_path);

        // Check Unit section
        assert!(
            unit.contains("[Unit]"),
            "Unit file should have [Unit] section"
        );
        assert!(
            unit.contains("Description=Lore AI session capture daemon"),
            "Unit file should have description"
        );
        assert!(
            unit.contains("After=default.target"),
            "Unit file should have After directive"
        );

        // Check Service section
        assert!(
            unit.contains("[Service]"),
            "Unit file should have [Service] section"
        );
        assert!(
            unit.contains("Type=simple"),
            "Unit file should have Type=simple"
        );
        assert!(
            unit.contains("ExecStart=/usr/local/bin/lore daemon start --foreground"),
            "Unit file should have correct ExecStart"
        );
        assert!(
            unit.contains("Restart=on-failure"),
            "Unit file should have Restart=on-failure"
        );
        assert!(
            unit.contains("RestartSec=5"),
            "Unit file should have RestartSec=5"
        );

        // Check Install section
        assert!(
            unit.contains("[Install]"),
            "Unit file should have [Install] section"
        );
        assert!(
            unit.contains("WantedBy=default.target"),
            "Unit file should have WantedBy directive"
        );
    }

    #[test]
    fn test_launchd_plist_path() {
        let path = get_launchd_plist_path();
        assert!(path.is_ok(), "Should get launchd plist path");

        let path = path.unwrap();
        assert!(
            path.to_string_lossy().contains("Library/LaunchAgents"),
            "Path should be in LaunchAgents directory"
        );
        assert!(
            path.to_string_lossy().contains("com.lore.daemon.plist"),
            "Path should end with com.lore.daemon.plist"
        );
    }

    #[test]
    fn test_systemd_unit_path() {
        let path = get_systemd_unit_path();
        assert!(path.is_ok(), "Should get systemd unit path");

        let path = path.unwrap();
        assert!(
            path.to_string_lossy().contains(".config/systemd/user"),
            "Path should be in systemd user directory"
        );
        assert!(
            path.to_string_lossy().contains("lore.service"),
            "Path should end with lore.service"
        );
    }

    #[test]
    fn test_generate_launchd_plist_handles_special_paths() {
        // Test with path containing spaces
        let exe_path = std::path::Path::new("/path/with spaces/lore");
        let logs_dir = std::path::Path::new("/home/user name/.lore/logs");

        let plist = generate_launchd_plist(exe_path, logs_dir);

        assert!(
            plist.contains("/path/with spaces/lore"),
            "Plist should preserve spaces in paths"
        );
        assert!(
            plist.contains("/home/user name/.lore/logs/daemon.log"),
            "Plist should preserve spaces in log path"
        );
    }

    #[test]
    fn test_generate_systemd_unit_handles_special_paths() {
        // Test with path containing spaces
        let exe_path = std::path::Path::new("/path/with spaces/lore");

        let unit = generate_systemd_unit(exe_path);

        assert!(
            unit.contains("/path/with spaces/lore"),
            "Unit file should preserve spaces in paths"
        );
    }
}

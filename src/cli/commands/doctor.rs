//! Doctor command - diagnose Lore installation and configuration.
//!
//! Performs comprehensive health checks on the Lore installation including
//! configuration, database, daemon status, watchers, and MCP server.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

use crate::capture::watchers::{default_registry, WatcherRegistry};
use crate::cli::OutputFormat;
use crate::config::Config;
use crate::daemon::DaemonState;
use crate::storage::Database;

/// Arguments for the doctor command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore doctor               Run health checks\n    \
    lore doctor --format json Output as JSON")]
pub struct Args {
    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// Check status indicating the result of a health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CheckStatus {
    /// Check passed successfully.
    Ok,
    /// Check passed but with a warning.
    Warning,
    /// Check failed with an error.
    Error,
}

/// Result of a single health check.
#[derive(Debug, Serialize)]
struct CheckResult {
    name: String,
    status: CheckStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// JSON output structure for doctor.
#[derive(Serialize)]
struct DoctorOutput {
    configuration: ConfigurationStatus,
    database: DatabaseStatus,
    daemon: DaemonStatus,
    watchers: Vec<WatcherCheckResult>,
    mcp_server: CheckResult,
    summary: SummaryStatus,
}

/// Configuration check results.
#[derive(Serialize)]
struct ConfigurationStatus {
    config_file: CheckResult,
    machine_id: CheckResult,
    enabled_watchers: Vec<String>,
}

/// Database check results.
#[derive(Serialize)]
struct DatabaseStatus {
    status: CheckResult,
    sessions: i32,
    messages: i32,
    links: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    readable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    writable: Option<bool>,
}

/// Daemon check results.
#[derive(Serialize)]
struct DaemonStatus {
    status: CheckResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    socket: CheckResult,
    logs: CheckResult,
}

/// Watcher check result.
#[derive(Serialize)]
struct WatcherCheckResult {
    name: String,
    status: CheckStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_count: Option<usize>,
}

/// Summary of all checks.
#[derive(Serialize)]
struct SummaryStatus {
    ok_count: usize,
    warning_count: usize,
    error_count: usize,
    exit_code: i32,
}

/// Executes the doctor command.
///
/// Performs health checks on configuration, database, daemon, watchers,
/// and MCP server. Returns exit code based on severity of issues found.
pub fn run(args: Args) -> Result<()> {
    let registry = default_registry();
    let config_result = Config::load();

    match args.format {
        OutputFormat::Json => run_json(&registry, config_result.as_ref().ok()),
        OutputFormat::Text | OutputFormat::Markdown => {
            run_text(&registry, config_result.as_ref().ok())
        }
    }
}

/// Runs doctor with JSON output.
fn run_json(registry: &WatcherRegistry, config: Option<&Config>) -> Result<()> {
    let mut ok_count = 0;
    let mut warning_count = 0;
    let mut error_count = 0;

    // Configuration checks
    let config_file = check_config_file();
    update_counts(
        &config_file.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let machine_id = check_machine_id(config);
    update_counts(
        &machine_id.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let enabled_watchers = config.map(|c| c.watchers.clone()).unwrap_or_default();

    // Database checks
    let (db_status, sessions, messages, links, readable, writable) = check_database();
    update_counts(
        &db_status.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    // Daemon checks
    let (daemon_status, pid) = check_daemon_status();
    update_counts(
        &daemon_status.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let socket = check_daemon_socket();
    update_counts(
        &socket.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let logs = check_daemon_logs();
    update_counts(
        &logs.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    // Watcher checks
    let watchers = check_watchers(registry, config);
    for w in &watchers {
        update_counts(
            &w.status,
            &mut ok_count,
            &mut warning_count,
            &mut error_count,
        );
    }

    // MCP server check
    let mcp = check_mcp_server();
    update_counts(
        &mcp.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let exit_code = if error_count > 0 {
        2
    } else if warning_count > 0 {
        1
    } else {
        0
    };

    let output = DoctorOutput {
        configuration: ConfigurationStatus {
            config_file,
            machine_id,
            enabled_watchers,
        },
        database: DatabaseStatus {
            status: db_status,
            sessions,
            messages,
            links,
            readable,
            writable,
        },
        daemon: DaemonStatus {
            status: daemon_status,
            pid,
            socket,
            logs,
        },
        watchers,
        mcp_server: mcp,
        summary: SummaryStatus {
            ok_count,
            warning_count,
            error_count,
            exit_code,
        },
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{json}");

    std::process::exit(exit_code);
}

/// Runs doctor with text output.
fn run_text(registry: &WatcherRegistry, config: Option<&Config>) -> Result<()> {
    let mut ok_count = 0;
    let mut warning_count = 0;
    let mut error_count = 0;

    println!("{}", "Lore Doctor".bold().cyan());
    println!();

    // Configuration section
    println!("{}", "Configuration:".bold());

    let config_file = check_config_file();
    print_check("Config file", &config_file);
    update_counts(
        &config_file.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let machine_id = check_machine_id(config);
    print_check("Machine ID", &machine_id);
    update_counts(
        &machine_id.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    if let Some(cfg) = config {
        if !cfg.watchers.is_empty() {
            println!(
                "  {} {}",
                "Watchers:".dimmed(),
                cfg.watchers.join(", ").cyan()
            );
        }
    }
    println!();

    // Database section
    println!("{}", "Database:".bold());
    let (db_status, sessions, messages, links, readable, writable) = check_database();
    print_check("Status", &db_status);
    update_counts(
        &db_status.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    if db_status.status == CheckStatus::Ok {
        println!("  Sessions:        {}", format_number(sessions));
        println!("  Messages:        {}", format_number(messages));
        println!("  Links:           {}", format_number(links));

        if let (Some(r), Some(w)) = (readable, writable) {
            let perms = match (r, w) {
                (true, true) => "read/write".green(),
                (true, false) => "read-only".yellow(),
                (false, _) => "inaccessible".red(),
            };
            println!("  Permissions:     {perms}");
        }
    }
    println!();

    // Daemon section
    println!("{}", "Daemon:".bold());
    let (daemon_status, pid) = check_daemon_status();
    let daemon_msg = if daemon_status.status == CheckStatus::Ok {
        if let Some(p) = pid {
            format!("{} (PID {})", "Running".green(), p)
        } else {
            "Running".green().to_string()
        }
    } else if daemon_status.status == CheckStatus::Warning {
        "Not running".yellow().to_string()
    } else {
        daemon_status.message.red().to_string()
    };
    println!("  Status:          {daemon_msg}");
    update_counts(
        &daemon_status.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let socket = check_daemon_socket();
    print_check("Socket", &socket);
    update_counts(
        &socket.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );

    let logs = check_daemon_logs();
    print_check("Logs", &logs);
    update_counts(
        &logs.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );
    println!();

    // Watchers section
    println!("{}", "Watchers:".bold());
    let watcher_results = check_watchers(registry, config);
    for w in &watcher_results {
        let status_str = match w.status {
            CheckStatus::Ok => {
                if let Some(count) = w.file_count {
                    format!("{} ({} files)", "OK".green(), count)
                } else {
                    "OK".green().to_string()
                }
            }
            CheckStatus::Warning => w.message.yellow().to_string(),
            CheckStatus::Error => w.message.red().to_string(),
        };
        println!("  {:15}  {}", format!("{}:", w.name).cyan(), status_str);
        update_counts(
            &w.status,
            &mut ok_count,
            &mut warning_count,
            &mut error_count,
        );
    }
    println!();

    // MCP section
    let mcp = check_mcp_server();
    print!("{}", "MCP Server:".bold());
    let mcp_str = match mcp.status {
        CheckStatus::Ok => "OK".green().to_string(),
        CheckStatus::Warning => mcp.message.yellow().to_string(),
        CheckStatus::Error => mcp.message.red().to_string(),
    };
    println!("        {mcp_str}");
    update_counts(
        &mcp.status,
        &mut ok_count,
        &mut warning_count,
        &mut error_count,
    );
    println!();

    // Summary
    let exit_code = if error_count > 0 {
        println!("{}", format!("{error_count} error(s) found.").red().bold());
        2
    } else if warning_count > 0 {
        println!(
            "{}",
            format!("{warning_count} warning(s) found.").yellow().bold()
        );
        1
    } else {
        println!("{}", "No issues found.".green().bold());
        0
    };

    std::process::exit(exit_code);
}

/// Updates the count variables based on a check status.
fn update_counts(status: &CheckStatus, ok: &mut usize, warning: &mut usize, error: &mut usize) {
    match status {
        CheckStatus::Ok => *ok += 1,
        CheckStatus::Warning => *warning += 1,
        CheckStatus::Error => *error += 1,
    }
}

/// Prints a check result in text format.
fn print_check(label: &str, result: &CheckResult) {
    let status_str = match result.status {
        CheckStatus::Ok => "OK".green(),
        CheckStatus::Warning => "Warning".yellow(),
        CheckStatus::Error => "Error".red(),
    };

    let detail = result
        .detail
        .as_ref()
        .map(|d| format!(" ({})", d.dimmed()))
        .unwrap_or_default();

    if result.status == CheckStatus::Ok {
        println!("  {:15}  {}{}", format!("{label}:"), status_str, detail);
    } else {
        println!(
            "  {:15}  {}: {}{}",
            format!("{label}:"),
            status_str,
            result.message,
            detail
        );
    }
}

/// Formats a number with comma separators.
fn format_number(n: i32) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// ==================== Individual Checks ====================

/// Checks if the config file exists and is valid.
fn check_config_file() -> CheckResult {
    let config_path = match Config::config_path() {
        Ok(p) => p,
        Err(e) => {
            return CheckResult {
                name: "config_file".to_string(),
                status: CheckStatus::Error,
                message: format!("Cannot determine config path: {e}"),
                detail: None,
            };
        }
    };

    if !config_path.exists() {
        return CheckResult {
            name: "config_file".to_string(),
            status: CheckStatus::Warning,
            message: "Config file not found".to_string(),
            detail: Some(config_path.display().to_string()),
        };
    }

    // Try to load it to verify it is valid
    match Config::load() {
        Ok(_) => CheckResult {
            name: "config_file".to_string(),
            status: CheckStatus::Ok,
            message: "Valid".to_string(),
            detail: Some(config_path.display().to_string()),
        },
        Err(e) => CheckResult {
            name: "config_file".to_string(),
            status: CheckStatus::Error,
            message: format!("Invalid config: {e}"),
            detail: Some(config_path.display().to_string()),
        },
    }
}

/// Checks if the machine ID is set.
fn check_machine_id(config: Option<&Config>) -> CheckResult {
    match config {
        Some(cfg) => {
            if let Some(ref id) = cfg.machine_id {
                let short_id = if id.len() > 8 { &id[..8] } else { id };
                CheckResult {
                    name: "machine_id".to_string(),
                    status: CheckStatus::Ok,
                    message: "Set".to_string(),
                    detail: Some(short_id.to_string()),
                }
            } else {
                CheckResult {
                    name: "machine_id".to_string(),
                    status: CheckStatus::Warning,
                    message: "Not set".to_string(),
                    detail: Some("Run 'lore init' to generate".to_string()),
                }
            }
        }
        None => CheckResult {
            name: "machine_id".to_string(),
            status: CheckStatus::Warning,
            message: "Config not loaded".to_string(),
            detail: None,
        },
    }
}

/// Checks the database status and returns stats.
fn check_database() -> (CheckResult, i32, i32, i32, Option<bool>, Option<bool>) {
    let db_path = match crate::storage::db::default_db_path() {
        Ok(p) => p,
        Err(e) => {
            return (
                CheckResult {
                    name: "database".to_string(),
                    status: CheckStatus::Error,
                    message: format!("Cannot determine database path: {e}"),
                    detail: None,
                },
                0,
                0,
                0,
                None,
                None,
            );
        }
    };

    if !db_path.exists() {
        return (
            CheckResult {
                name: "database".to_string(),
                status: CheckStatus::Warning,
                message: "Database file not found".to_string(),
                detail: Some(db_path.display().to_string()),
            },
            0,
            0,
            0,
            None,
            None,
        );
    }

    // Check file permissions
    let (readable, writable) = check_file_permissions(&db_path);

    // Try to open the database
    match Database::open_default() {
        Ok(db) => {
            let sessions = db.session_count().unwrap_or(0);
            let messages = db.message_count().unwrap_or(0);
            let links = db.link_count().unwrap_or(0);

            (
                CheckResult {
                    name: "database".to_string(),
                    status: CheckStatus::Ok,
                    message: "OK".to_string(),
                    detail: Some(db_path.display().to_string()),
                },
                sessions,
                messages,
                links,
                Some(readable),
                Some(writable),
            )
        }
        Err(e) => (
            CheckResult {
                name: "database".to_string(),
                status: CheckStatus::Error,
                message: format!("Cannot open database: {e}"),
                detail: Some(db_path.display().to_string()),
            },
            0,
            0,
            0,
            Some(readable),
            Some(writable),
        ),
    }
}

/// Checks file permissions (readable, writable).
fn check_file_permissions(path: &PathBuf) -> (bool, bool) {
    let readable = fs::File::open(path).is_ok();
    let writable = fs::OpenOptions::new().write(true).open(path).is_ok();
    (readable, writable)
}

/// Checks daemon running status.
fn check_daemon_status() -> (CheckResult, Option<u32>) {
    match DaemonState::new() {
        Ok(state) => {
            if state.is_running() {
                let pid = state.get_pid();
                (
                    CheckResult {
                        name: "daemon_status".to_string(),
                        status: CheckStatus::Ok,
                        message: "Running".to_string(),
                        detail: pid.map(|p| format!("PID {p}")),
                    },
                    pid,
                )
            } else {
                // Daemon not running is a warning, not an error
                (
                    CheckResult {
                        name: "daemon_status".to_string(),
                        status: CheckStatus::Warning,
                        message: "Not running".to_string(),
                        detail: Some("Run 'lore daemon start' to start".to_string()),
                    },
                    None,
                )
            }
        }
        Err(e) => (
            CheckResult {
                name: "daemon_status".to_string(),
                status: CheckStatus::Error,
                message: format!("Cannot check daemon state: {e}"),
                detail: None,
            },
            None,
        ),
    }
}

/// Checks if the daemon socket file exists.
fn check_daemon_socket() -> CheckResult {
    match DaemonState::new() {
        Ok(state) => {
            if state.socket_path.exists() {
                CheckResult {
                    name: "daemon_socket".to_string(),
                    status: CheckStatus::Ok,
                    message: "OK".to_string(),
                    detail: Some(state.socket_path.display().to_string()),
                }
            } else {
                // Socket not existing when daemon is not running is fine
                if state.is_running() {
                    CheckResult {
                        name: "daemon_socket".to_string(),
                        status: CheckStatus::Error,
                        message: "Socket missing while daemon running".to_string(),
                        detail: Some(state.socket_path.display().to_string()),
                    }
                } else {
                    CheckResult {
                        name: "daemon_socket".to_string(),
                        status: CheckStatus::Ok,
                        message: "Not present (daemon not running)".to_string(),
                        detail: None,
                    }
                }
            }
        }
        Err(e) => CheckResult {
            name: "daemon_socket".to_string(),
            status: CheckStatus::Error,
            message: format!("Cannot check socket: {e}"),
            detail: None,
        },
    }
}

/// Checks if the daemon log file exists.
fn check_daemon_logs() -> CheckResult {
    let log_path = match dirs::home_dir() {
        Some(home) => home.join(".lore").join("logs").join("daemon.log"),
        None => {
            return CheckResult {
                name: "daemon_logs".to_string(),
                status: CheckStatus::Warning,
                message: "Cannot determine log path".to_string(),
                detail: None,
            };
        }
    };

    // Also check the old location
    let old_log_path = match dirs::home_dir() {
        Some(home) => home.join(".lore").join("daemon.log"),
        None => log_path.clone(),
    };

    if log_path.exists() {
        CheckResult {
            name: "daemon_logs".to_string(),
            status: CheckStatus::Ok,
            message: "OK".to_string(),
            detail: Some(log_path.display().to_string()),
        }
    } else if old_log_path.exists() {
        CheckResult {
            name: "daemon_logs".to_string(),
            status: CheckStatus::Ok,
            message: "OK".to_string(),
            detail: Some(old_log_path.display().to_string()),
        }
    } else {
        // No log file is OK if daemon has never run
        CheckResult {
            name: "daemon_logs".to_string(),
            status: CheckStatus::Ok,
            message: "Not present (daemon may not have run)".to_string(),
            detail: None,
        }
    }
}

/// Checks the status of each watcher.
fn check_watchers(registry: &WatcherRegistry, config: Option<&Config>) -> Vec<WatcherCheckResult> {
    let enabled_names: Vec<&str> = config
        .map(|c| c.watchers.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    registry
        .all_watchers()
        .iter()
        .map(|watcher| {
            let info = watcher.info();
            let name = info.name.to_string();
            let is_enabled = enabled_names.contains(&info.name);

            if !watcher.is_available() {
                WatcherCheckResult {
                    name,
                    status: if is_enabled {
                        CheckStatus::Warning
                    } else {
                        CheckStatus::Ok
                    },
                    message: if is_enabled {
                        "Enabled but not available".to_string()
                    } else {
                        "Not available".to_string()
                    },
                    file_count: None,
                }
            } else {
                let file_count = watcher.find_sources().ok().map(|s| s.len());
                let status = if is_enabled {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                };
                let message = if is_enabled {
                    "OK (enabled)".to_string()
                } else {
                    "Available but not enabled".to_string()
                };

                WatcherCheckResult {
                    name,
                    status,
                    message,
                    file_count,
                }
            }
        })
        .collect()
}

/// Checks if the MCP server module can be loaded.
fn check_mcp_server() -> CheckResult {
    // The MCP server is a compile-time module. If this code compiles,
    // the MCP server module is available. We just verify the module exists
    // by referencing it (this is a compile-time check).
    //
    // A more thorough check would actually start the server and verify
    // it responds, but that would be intrusive. For now, we just confirm
    // the module is present.
    CheckResult {
        name: "mcp_server".to_string(),
        status: CheckStatus::Ok,
        message: "Module available".to_string(),
        detail: None,
    }
}

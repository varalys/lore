//! Init command - guided first-run setup for Lore.
//!
//! Detects installed AI coding tools and creates initial configuration.

use anyhow::{Context, Result};
use colored::Colorize;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::capture::watchers::aider::scan_directories_for_aider_files;
use crate::capture::watchers::{default_registry, Watcher, WatcherRegistry};
use crate::cli::commands::{completions, import};
use crate::config::Config;
use crate::daemon::DaemonState;
use crate::storage::db::default_db_path;
use crate::storage::{Database, Machine};
use crate::summarize::provider::{default_model, SummaryProviderKind};
use clap::CommandFactory;

/// Arguments for the init command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore init              Run guided setup\n    \
    lore init --force      Reinitialize even if already configured")]
pub struct Args {
    /// Reinitialize configuration even if Lore is already set up
    #[arg(short, long)]
    pub force: bool,
}

/// Information about a detected AI coding tool.
#[derive(Debug)]
struct DetectedTool {
    /// Watcher name (used in config)
    name: String,
    /// Human-readable description
    description: String,
    /// Whether session files were found
    has_sessions: bool,
    /// Number of session files found
    session_count: usize,
}

/// Executes the init command.
///
/// Guides the user through initial Lore setup:
/// 1. Checks if already initialized
/// 2. Auto-detects installed AI coding tools
/// 3. Prompts user to confirm or customize watcher selection
/// 4. Creates configuration file with selected watchers
pub fn run(args: Args) -> Result<()> {
    println!("{}", "Lore Setup".bold().cyan());
    println!("{}", "Reasoning history for code".dimmed());
    println!();

    // Check if already initialized
    let db_path = default_db_path()?;
    let config_path = Config::config_path()?;
    let already_initialized = db_path.exists() || config_path.exists();

    if already_initialized && !args.force {
        println!("{}", "Lore is already initialized.".yellow());
        println!();
        println!("  Database:    {}", db_path.display());
        println!("  Config:      {}", config_path.display());
        println!();
        println!("Use {} to reconfigure.", "lore init --force".cyan());
        return Ok(());
    }

    if already_initialized && args.force {
        println!("{}", "Reinitializing Lore configuration...".yellow());
        println!();
    }

    // Detect installed AI coding tools
    println!("{}", "Detecting installed AI coding tools...".bold());

    let registry = default_registry();
    let detected = detect_tools(&registry);

    println!();

    if detected.is_empty() {
        println!("  {}", "No AI coding tools detected.".dimmed());
        println!();
        println!(
            "Lore supports: Claude Code, Aider, Continue.dev, Cline, Codex, Gemini CLI, and more."
        );
        println!(
            "Install one of these tools and run {} again.",
            "lore init".cyan()
        );
        return Ok(());
    }

    // Show detected tools
    let tools_with_sessions: Vec<&DetectedTool> =
        detected.iter().filter(|t| t.has_sessions).collect();
    let tools_without_sessions: Vec<&DetectedTool> =
        detected.iter().filter(|t| !t.has_sessions).collect();

    if !tools_with_sessions.is_empty() {
        println!("  {} with existing sessions:", "Found".green());
        for tool in &tools_with_sessions {
            println!(
                "    {} - {} ({} session files)",
                tool.name.cyan(),
                tool.description,
                tool.session_count
            );
        }
    }

    if !tools_without_sessions.is_empty() {
        println!();
        println!("  {} (no sessions yet):", "Available".dimmed());
        for tool in &tools_without_sessions {
            println!("    {} - {}", tool.name.cyan(), tool.description.dimmed());
        }
    }

    println!();

    // Prompt user to confirm watcher selection
    let selected_watchers = prompt_watcher_selection(&detected)?;

    if selected_watchers.is_empty() {
        println!();
        println!("{}", "No watchers selected. Setup cancelled.".yellow());
        return Ok(());
    }

    // Create configuration
    println!();
    println!("{}", "Creating configuration...".bold());

    let mut config = Config {
        watchers: selected_watchers.clone(),
        ..Config::default()
    };

    // Ensure the config directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    // Configure machine identity
    println!();
    println!("{}", "Machine Identity".bold());
    let machine_id = config.get_or_create_machine_id()?;
    println!("  ID: {}", machine_id.dimmed());

    let detected_hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());
    println!("  Detected hostname: {}", detected_hostname.cyan());
    println!();

    let prompt_text = format!(
        "What would you like to call this machine? [{}]",
        detected_hostname
    );
    print!("{}: ", prompt_text);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    let machine_name = if input.is_empty() {
        detected_hostname
    } else {
        input.to_string()
    };

    config.set_machine_name(&machine_name)?;
    println!("Machine name set to: {}", machine_name.green());
    println!();

    config
        .save_to_path(&config_path)
        .context("Failed to save configuration")?;

    println!("  Created: {}", config_path.display());

    // Initialize database if needed
    let db_created = !db_path.exists();
    let db = crate::storage::Database::open_default()?;
    if db_created {
        println!("  Created: {}", db_path.display());
    }

    // Register this machine in the machines table for cloud sync
    let machine = Machine {
        id: machine_id.clone(),
        name: machine_name.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db.upsert_machine(&machine)?;

    println!();
    println!("Enabled watchers: {}", selected_watchers.join(", ").cyan());

    // Check if there are any sessions to import
    let tools_with_sessions: Vec<_> = detected.iter().filter(|t| t.has_sessions).collect();
    let total_sessions: usize = tools_with_sessions.iter().map(|t| t.session_count).sum();
    let has_sessions = !tools_with_sessions.is_empty();

    if has_sessions {
        println!();
        let prompt = format!(
            "Import existing sessions now? ({} sessions from {} tools)",
            total_sessions,
            tools_with_sessions.len()
        );
        if prompt_yes_no(&prompt, true)? {
            println!();
            let stats = import::run_import(false, false)?;
            println!();
            println!(
                "{}",
                format!(
                    "Imported {} sessions from {} tools",
                    stats.imported, stats.tools_count
                )
                .bold()
            );
            if stats.skipped > 0 || stats.errors > 0 {
                println!("  ({} skipped, {} errors)", stats.skipped, stats.errors);
            }
        }
    }

    // Offer to scan for additional aider projects if aider was enabled
    let aider_enabled = selected_watchers.iter().any(|w| w == "aider");
    if aider_enabled {
        offer_aider_scan(&db)?;
    }

    // Offer to configure session summaries
    println!();
    offer_summary_setup(&mut config, &config_path)?;

    // Offer to install shell completions
    println!();
    offer_completions_install()?;

    // Offer to start background service
    println!();
    offer_service_install()?;

    println!();
    println!("{}", "Setup complete!".green().bold());
    println!();
    println!("Next steps:");
    if !has_sessions {
        println!("  {} - Import existing sessions", "lore import".cyan());
    }
    println!("  {} - Check current status", "lore status".cyan());
    println!("  {} - View configuration", "lore config".cyan());

    Ok(())
}

/// Detects installed AI coding tools.
///
/// Checks for each registered watcher whether it is available and
/// whether session files exist.
fn detect_tools(registry: &WatcherRegistry) -> Vec<DetectedTool> {
    let mut detected = Vec::new();

    for watcher in registry.all_watchers() {
        let info = watcher.info();
        let available = watcher.is_available();

        // Only include available watchers
        if !available {
            continue;
        }

        let sources = watcher.find_sources().unwrap_or_default();
        let session_count = sources.len();
        let has_sessions = !sources.is_empty();

        detected.push(DetectedTool {
            name: info.name.to_string(),
            description: info.description.to_string(),
            has_sessions,
            session_count,
        });
    }

    detected
}

/// Prompts the user to confirm or customize watcher selection.
///
/// Shows detected tools and asks user which ones to enable.
/// Returns the list of selected watcher names.
fn prompt_watcher_selection(detected: &[DetectedTool]) -> Result<Vec<String>> {
    // Default: enable all watchers with sessions, plus any that are available
    let default_selection: Vec<String> = detected.iter().map(|t| t.name.clone()).collect();

    println!("{}", "Which tools would you like to enable?".bold());
    println!();

    // Show numbered options
    for (i, tool) in detected.iter().enumerate() {
        let num = i + 1;
        let status = if tool.has_sessions {
            format!("({} sessions)", tool.session_count)
                .green()
                .to_string()
        } else {
            "(no sessions yet)".dimmed().to_string()
        };
        println!(
            "  [{}] {} - {} {}",
            num,
            tool.name.cyan(),
            tool.description,
            status
        );
    }

    println!();
    println!("Enter tool numbers separated by commas, or press Enter to enable all:");
    print!("{}", "> ".cyan());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    // If empty, use all detected watchers
    if input.is_empty() {
        return Ok(default_selection);
    }

    // Parse user selection
    let mut selected = Vec::new();
    for part in input.split(',') {
        let part = part.trim();

        // Try to parse as number
        if let Ok(num) = part.parse::<usize>() {
            if num >= 1 && num <= detected.len() {
                selected.push(detected[num - 1].name.clone());
            } else {
                println!("{}: {} is not a valid option", "Warning".yellow(), num);
            }
        } else if !part.is_empty() {
            // Try to match by name
            if let Some(tool) = detected.iter().find(|t| t.name == part) {
                selected.push(tool.name.clone());
            } else {
                println!(
                    "{}: '{}' is not a recognized tool",
                    "Warning".yellow(),
                    part
                );
            }
        }
    }

    // Remove duplicates while preserving order
    let mut seen = std::collections::HashSet::new();
    selected.retain(|x| seen.insert(x.clone()));

    Ok(selected)
}

/// Prompts the user for a yes/no answer.
///
/// Displays the prompt with the default choice indicated. Returns the
/// user's choice, or the default if they press Enter without input.
fn prompt_yes_no(prompt: &str, default: bool) -> Result<bool> {
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    print!("{prompt} {hint} ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input.is_empty() {
        return Ok(default);
    }

    match input.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Ok(default),
    }
}

/// Offers to install shell completions for tab-completion.
///
/// Auto-detects the shell and installs completions if the user agrees.
/// If the shell cannot be detected, provides a message to run manually.
fn offer_completions_install() -> Result<()> {
    // Try to detect the shell
    let shell = match completions::detect_shell() {
        Some(s) => s,
        None => {
            println!(
                "{}",
                "Could not detect shell. Run 'lore completions install --shell <shell>' manually."
                    .dimmed()
            );
            return Ok(());
        }
    };

    let shell_name = match shell {
        clap_complete::Shell::Bash => "bash",
        clap_complete::Shell::Zsh => "zsh",
        clap_complete::Shell::Fish => "fish",
        clap_complete::Shell::PowerShell => "PowerShell",
        clap_complete::Shell::Elvish => "elvish",
        _ => "shell",
    };

    let prompt = format!(
        "Install shell completions for tab-completion? ({})",
        shell_name
    );

    if !prompt_yes_no(&prompt, true)? {
        return Ok(());
    }

    // Get the CLI command for generating completions
    // We need to import the Cli struct - it is re-exported from main
    #[derive(clap::Parser)]
    #[command(name = "lore")]
    struct LoreCli {
        #[command(subcommand)]
        command: LoreCommand,
    }

    #[derive(clap::Subcommand)]
    enum LoreCommand {
        Init,
        Status,
        Sessions,
        Show,
        Link,
        Unlink,
        Delete,
        Search,
        Config,
        Import,
        Hooks,
        Daemon,
        Db,
        Completions,
    }

    let mut cmd = LoreCli::command();

    match completions::install_completions(&mut cmd, shell) {
        Ok(path) => {
            println!("Detected shell: {}", shell_name.cyan());
            println!(
                "Completions installed to: {}",
                path.display().to_string().cyan()
            );

            // Show activation instructions
            let instructions = match shell {
                clap_complete::Shell::Bash => {
                    format!("Restart your shell or run: source {}", path.display())
                }
                clap_complete::Shell::Zsh => {
                    "Restart your shell or run: autoload -Uz compinit && compinit".to_string()
                }
                clap_complete::Shell::Fish => {
                    format!("Restart your shell or run: source {}", path.display())
                }
                clap_complete::Shell::PowerShell => {
                    format!("Restart PowerShell or run: . {}", path.display())
                }
                clap_complete::Shell::Elvish => "Restart elvish or run: use lore".to_string(),
                _ => "Restart your shell to activate completions.".to_string(),
            };
            println!("{}", instructions.dimmed());
        }
        Err(e) => {
            println!(
                "{}: {}",
                "Warning".yellow(),
                format!("Could not install completions: {}", e).dimmed()
            );
            println!(
                "{}",
                "Run 'lore completions install' manually later.".dimmed()
            );
        }
    }

    Ok(())
}

/// Operating system types for service management.
///
/// Some variants may appear unused on certain platforms but are required
/// for cross-platform compilation support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum OperatingSystem {
    /// macOS - uses brew services
    MacOS,
    /// Linux - uses systemd user services
    Linux,
    /// Windows - service management not yet supported
    Windows,
    /// Unknown operating system
    Unknown,
}

impl std::fmt::Display for OperatingSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperatingSystem::MacOS => write!(f, "macOS"),
            OperatingSystem::Linux => write!(f, "Linux"),
            OperatingSystem::Windows => write!(f, "Windows"),
            OperatingSystem::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detects the current operating system.
///
/// Returns an enum variant indicating the detected OS type.
pub fn detect_os() -> OperatingSystem {
    #[cfg(target_os = "macos")]
    {
        OperatingSystem::MacOS
    }
    #[cfg(target_os = "linux")]
    {
        OperatingSystem::Linux
    }
    #[cfg(target_os = "windows")]
    {
        OperatingSystem::Windows
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        OperatingSystem::Unknown
    }
}

/// Offers to start the lore background service.
///
/// Detects the OS and provides appropriate service installation:
/// - macOS: Uses `brew services start lore`
/// - Linux: Creates systemd user service and enables it
fn offer_service_install() -> Result<()> {
    let os = detect_os();

    match os {
        OperatingSystem::MacOS => offer_macos_service()?,
        OperatingSystem::Linux => offer_linux_service()?,
        OperatingSystem::Windows => {
            println!(
                "{}",
                "Windows service management is not yet supported.".dimmed()
            );
            println!(
                "{}",
                "You can run 'lore daemon start' manually to start the daemon.".dimmed()
            );
        }
        OperatingSystem::Unknown => {
            println!(
                "{}",
                "Could not detect OS. You can run 'lore daemon start' manually.".dimmed()
            );
        }
    }

    Ok(())
}

/// Offers to start lore as a macOS service using Homebrew.
fn offer_macos_service() -> Result<()> {
    print_service_benefits();

    if !prompt_yes_no("Start lore as a background service?", true)? {
        print_service_declined_message(OperatingSystem::MacOS);
        return Ok(());
    }

    // Check if brew is available
    let brew_check = std::process::Command::new("brew").arg("--version").output();

    match brew_check {
        Ok(output) if output.status.success() => {
            println!("Starting lore service via Homebrew...");

            let result = std::process::Command::new("brew")
                .args(["services", "start", "lore"])
                .output();

            match result {
                Ok(output) if output.status.success() => {
                    println!("{}", "Lore background service started!".green());
                    println!("{}", "Sessions will now be captured in real-time.".dimmed());
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    // Check if it's just already running
                    if stderr.contains("already started") {
                        println!("{}", "Lore background service is already running.".green());
                        println!("{}", "Sessions will now be captured in real-time.".dimmed());
                    } else {
                        println!(
                            "{}: {}",
                            "Warning".yellow(),
                            "Failed to start service via brew".dimmed()
                        );
                        if !stderr.is_empty() {
                            println!("  {}", stderr.trim().dimmed());
                        }
                        fallback_to_daemon_install()?;
                    }
                }
                Err(e) => {
                    println!(
                        "{}: {}",
                        "Warning".yellow(),
                        format!("Failed to run brew: {}", e).dimmed()
                    );
                    fallback_to_daemon_install()?;
                }
            }
        }
        _ => {
            // Homebrew not available, use native daemon install
            fallback_to_daemon_install()?;
        }
    }

    Ok(())
}

/// Falls back to native daemon install when brew is not available.
fn fallback_to_daemon_install() -> Result<()> {
    println!(
        "{}",
        "Falling back to native service installation...".dimmed()
    );

    let lore_exe = std::env::current_exe().context("Could not determine lore binary path")?;

    let result = std::process::Command::new(&lore_exe)
        .args(["daemon", "install"])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            println!("{}", "Lore background service installed!".green());
            println!("{}", "Sessions will now be captured in real-time.".dimmed());
        }
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("already installed") {
                println!("{}", "Lore service is already installed.".green());
            } else {
                println!(
                    "{}",
                    "You can run 'lore daemon start' manually instead.".dimmed()
                );
            }
        }
        Err(e) => {
            println!(
                "{}: {}",
                "Warning".yellow(),
                format!("Failed to install service: {}", e).dimmed()
            );
            println!(
                "{}",
                "You can run 'lore daemon start' manually instead.".dimmed()
            );
        }
    }

    Ok(())
}

/// Offers to start lore as a Linux systemd user service.
fn offer_linux_service() -> Result<()> {
    print_service_benefits();

    if !prompt_yes_no("Start lore as a background service?", true)? {
        print_service_declined_message(OperatingSystem::Linux);
        return Ok(());
    }

    // Check if systemctl is available
    let systemctl_check = std::process::Command::new("systemctl")
        .arg("--version")
        .output();

    match systemctl_check {
        Ok(output) if output.status.success() => {
            // Check if daemon is already running (outside of systemd)
            // If so, we need to stop it before enabling systemd
            if let Ok(state) = DaemonState::new() {
                if state.is_running() {
                    if let Some(pid) = state.get_pid() {
                        println!(
                            "{}",
                            format!("Stopping existing daemon (PID {})...", pid).dimmed()
                        );
                        // Stop the existing daemon
                        let _ = std::process::Command::new("kill")
                            .arg(pid.to_string())
                            .output();
                        // Give it a moment to stop
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                }
            }

            // Create systemd user service directory and file
            if let Err(e) = create_systemd_service_file() {
                println!(
                    "{}: {}",
                    "Warning".yellow(),
                    format!("Failed to create service file: {}", e).dimmed()
                );
                println!(
                    "{}",
                    "You can run 'lore daemon start' manually instead.".dimmed()
                );
                return Ok(());
            }

            // Reload systemd user daemon
            let reload = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .output();

            if let Err(e) = reload {
                println!(
                    "{}: {}",
                    "Warning".yellow(),
                    format!("Failed to reload systemd: {}", e).dimmed()
                );
            }

            // Enable and start the service
            let result = std::process::Command::new("systemctl")
                .args(["--user", "enable", "--now", "lore"])
                .output();

            match result {
                Ok(output) if output.status.success() => {
                    println!("{}", "Lore background service started!".green());
                    println!("{}", "Sessions will now be captured in real-time.".dimmed());
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!(
                        "{}: {}",
                        "Warning".yellow(),
                        "Failed to enable service".dimmed()
                    );
                    if !stderr.is_empty() {
                        println!("  {}", stderr.trim().dimmed());
                    }
                    println!(
                        "{}",
                        "You can run 'lore daemon start' manually instead.".dimmed()
                    );
                }
                Err(e) => {
                    println!(
                        "{}: {}",
                        "Warning".yellow(),
                        format!("Failed to run systemctl: {}", e).dimmed()
                    );
                    println!(
                        "{}",
                        "You can run 'lore daemon start' manually instead.".dimmed()
                    );
                }
            }
        }
        _ => {
            println!("{}: {}", "Note".yellow(), "systemd not found".dimmed());
            println!(
                "{}",
                "You can run 'lore daemon start' manually to start the daemon.".dimmed()
            );
        }
    }

    Ok(())
}

/// Creates the systemd user service file for lore.
///
/// Creates the file at ~/.config/systemd/user/lore.service
fn create_systemd_service_file() -> Result<()> {
    let service_dir = dirs::config_dir()
        .context("Could not find config directory")?
        .join("systemd/user");

    std::fs::create_dir_all(&service_dir).context("Failed to create systemd user directory")?;

    let service_file = service_dir.join("lore.service");

    // Get the actual path to the lore binary (works whether installed via cargo or package manager)
    let lore_binary = std::env::current_exe().context("Could not determine lore binary path")?;
    let lore_binary_path = lore_binary.display();

    let service_content = format!(
        r#"[Unit]
Description=Lore - Reasoning history for code
Documentation=https://github.com/varalys/lore

[Service]
Type=simple
ExecStart={lore_binary_path} daemon start --foreground
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"#
    );

    std::fs::write(&service_file, &service_content)
        .with_context(|| format!("Failed to write service file: {}", service_file.display()))?;

    println!(
        "Created service file: {}",
        service_file.display().to_string().dimmed()
    );

    Ok(())
}

/// Prints the benefits of running lore as a background service.
///
/// This is shown before the prompt to help users understand what the
/// background service provides.
fn print_service_benefits() {
    println!("{}", "Background Service".bold());
    println!();
    println!("Lore can run as a background service to:");
    println!("  - Capture sessions in real-time as you work");
    println!("  - Auto-link sessions to commits when you commit");
    println!("  - Track branch changes automatically");
    println!();
}

/// Prints a message when the user declines service installation.
fn print_service_declined_message(os: OperatingSystem) {
    println!();
    println!("You can start the service later with:");
    match os {
        OperatingSystem::MacOS => {
            println!("  {}", "brew services start lore".cyan());
        }
        OperatingSystem::Linux => {
            println!("  {}", "systemctl --user enable --now lore".cyan());
        }
        _ => {
            println!("  {}", "lore daemon start".cyan());
        }
    }
}

/// Returns the path where Cursor stores its data.
///
/// - macOS: ~/Library/Application Support/Cursor/
/// - Linux: ~/.config/Cursor/
/// - Windows: %APPDATA%/Cursor/
#[allow(dead_code)]
pub fn cursor_data_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Cursor")
    }
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cursor")
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cursor")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cursor")
    }
}

/// Offers to scan additional directories for aider projects.
///
/// Aider stores `.aider.chat.history.md` files in project directories rather than
/// a central location. This prompts the user to scan additional directories beyond
/// the common project folders that are checked by default.
fn offer_aider_scan(db: &Database) -> Result<()> {
    println!();
    println!("{}", "Aider Projects".bold());
    println!();
    println!(
        "{}",
        "Aider stores chat history in project directories, not a central location.".dimmed()
    );
    println!(
        "{}",
        "Common folders (~/projects, ~/code, etc.) were already checked.".dimmed()
    );
    println!();

    if !prompt_yes_no("Scan additional directories for aider projects?", false)? {
        return Ok(());
    }

    // Loop to allow re-entry on invalid paths
    let valid_dirs = loop {
        println!();
        println!("Enter directories to scan (comma-separated), or press Enter to skip:");
        println!("{}", "  Examples: ~/projects, ~/code, ~/work".dimmed());
        print!("{}", "> ".cyan());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        // Empty input means skip
        if input.is_empty() {
            return Ok(());
        }

        // Parse directories - comma-separated only
        let parts: Vec<&str> = input.split(',').map(|s| s.trim()).collect();

        let directories: Vec<PathBuf> = parts
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| {
                // Expand ~ to home directory
                if let Some(rest) = s.strip_prefix("~/") {
                    if let Some(home) = dirs::home_dir() {
                        home.join(rest)
                    } else {
                        PathBuf::from(s)
                    }
                } else if s == "~" {
                    // Don't allow scanning entire home directory
                    PathBuf::from("~") // Will be caught as invalid
                } else {
                    PathBuf::from(s)
                }
            })
            .collect();

        if directories.is_empty() {
            println!("{}", "No directories specified.".yellow());
            return Ok(());
        }

        // Validate directories exist and collect invalid ones
        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for d in directories {
            if d.as_os_str() == "~" {
                println!(
                    "{}",
                    "Scanning entire home directory is not recommended.".yellow()
                );
                invalid.push(d);
            } else if d.exists() && d.is_dir() {
                valid.push(d);
            } else {
                invalid.push(d);
            }
        }

        // If all directories are valid, proceed
        if invalid.is_empty() {
            break valid;
        }

        // Show invalid directories
        println!();
        println!("{}", "Some directories were not found:".yellow());
        for d in &invalid {
            println!("  - {}", d.display());
        }

        if valid.is_empty() {
            // All were invalid - prompt to re-enter
            println!();
            println!("Please re-enter directories (comma-separated), or press Enter to skip.");
            continue;
        }

        // Some valid, some invalid - ask if they want to continue with valid ones
        println!();
        println!(
            "Found {} valid director{}:",
            valid.len(),
            if valid.len() == 1 { "y" } else { "ies" }
        );
        for d in &valid {
            println!("  - {}", d.display());
        }
        println!();

        if prompt_yes_no("Continue with these directories?", true)? {
            break valid;
        }

        // User declined - let them re-enter
        println!("Please re-enter directories (comma-separated), or press Enter to skip.");
    };

    println!();
    println!("{}", "Scanning for aider projects...".bold());

    // Track the last printed line length for clearing
    let mut last_line_len = 0;

    // Scan with progress display
    let found_files = scan_directories_for_aider_files(&valid_dirs, |current_dir, found_count| {
        // Create progress line
        let dir_display = current_dir
            .to_string_lossy()
            .chars()
            .take(60)
            .collect::<String>();
        let line = format!(
            "  {} {} [{}]",
            "scanning".dimmed(),
            dir_display,
            format!("{} found", found_count).green()
        );

        // Clear previous line and print new one
        print!("\r{:width$}\r", "", width = last_line_len);
        print!("{}", line);
        io::stdout().flush().ok();
        last_line_len = line.len();
    });

    // Clear the progress line
    print!("\r{:width$}\r", "", width = last_line_len);
    io::stdout().flush().ok();

    if found_files.is_empty() {
        println!("  {}", "No additional aider projects found.".dimmed());
        return Ok(());
    }

    println!(
        "  Found {} aider project(s)",
        found_files.len().to_string().green()
    );

    // Import the found files
    let watcher = crate::capture::watchers::aider::AiderWatcher;
    let mut imported = 0;
    let mut skipped = 0;

    for file_path in &found_files {
        match watcher.parse_source(file_path) {
            Ok(sessions) => {
                for (session, messages) in sessions {
                    // Check if already imported
                    if db.get_session(&session.id).ok().flatten().is_some() {
                        skipped += 1;
                        continue;
                    }

                    // Import the session
                    if let Err(e) = db.insert_session(&session) {
                        println!("  {}: Failed to import session: {}", "Warning".yellow(), e);
                        continue;
                    }

                    for message in &messages {
                        if let Err(e) = db.insert_message(message) {
                            println!("  {}: Failed to import message: {}", "Warning".yellow(), e);
                        }
                    }

                    imported += 1;
                }
            }
            Err(e) => {
                println!(
                    "  {}: Failed to parse {}: {}",
                    "Warning".yellow(),
                    file_path.display(),
                    e
                );
            }
        }
    }

    if imported > 0 {
        println!(
            "  Imported {} aider session(s)",
            imported.to_string().green()
        );
    }
    if skipped > 0 {
        println!("  ({} already imported)", skipped);
    }

    Ok(())
}

/// Offers to configure session summary generation during init.
///
/// Walks the user through selecting an LLM provider, entering an API key,
/// choosing a model, and enabling auto-summarize. Saves all settings to
/// the config file.
fn offer_summary_setup(config: &mut Config, config_path: &std::path::Path) -> Result<()> {
    println!("{}", "Session Summaries".bold());
    println!();
    println!("Lore can generate short summaries of your AI coding sessions using an LLM.");
    println!("Summaries help you quickly understand what each session accomplished.");
    println!("{}", "Requires an API key from a supported provider.".dimmed());
    println!();

    if !prompt_yes_no("Set up session summaries?", false)? {
        return Ok(());
    }

    println!();
    let provider = match prompt_provider_selection()? {
        Some(p) => p,
        None => return Ok(()),
    };

    println!();
    let api_key = prompt_api_key(&provider)?;
    if api_key.is_empty() {
        println!("{}", "No API key entered. Summary setup skipped.".yellow());
        return Ok(());
    }

    // Parse provider kind to get the default model
    let kind: SummaryProviderKind = provider
        .parse()
        .map_err(|e: String| anyhow::anyhow!("{}", e))?;
    let default = default_model(kind);

    println!();
    print!("Model [{}]: ", default.cyan());
    io::stdout().flush()?;

    let mut model_input = String::new();
    io::stdin().read_line(&mut model_input)?;
    let model_input = model_input.trim().to_string();

    println!();
    let auto_summarize = prompt_yes_no("Enable auto-summarize for new sessions?", true)?;

    // Apply all settings
    config.set("summary_provider", &provider)?;
    config.set(&format!("summary_api_key_{}", provider), &api_key)?;
    if !model_input.is_empty() {
        config.set(&format!("summary_model_{}", provider), &model_input)?;
    }
    config.set(
        "summary_auto",
        if auto_summarize { "true" } else { "false" },
    )?;
    config.set("summary_auto_threshold", "4")?;

    config
        .save_to_path(config_path)
        .context("Failed to save summary configuration")?;

    // Show confirmation
    let display_model = if model_input.is_empty() {
        default.to_string()
    } else {
        model_input
    };

    println!();
    println!("{}", "Summary configuration saved:".green());
    println!(
        "  Provider:       {}",
        capitalize_provider(&provider).cyan()
    );
    println!("  Model:          {}", display_model.cyan());
    println!("  API key:        {}", "(saved)".dimmed());
    println!(
        "  Auto-summarize: {}",
        if auto_summarize {
            "enabled".green().to_string()
        } else {
            "disabled".dimmed().to_string()
        }
    );

    Ok(())
}

/// Prompts the user to select an LLM provider from a numbered list.
///
/// Displays available providers and returns the lowercase provider name,
/// or `None` if the selection is invalid.
fn prompt_provider_selection() -> Result<Option<String>> {
    println!("{}", "Choose a provider:".bold());
    println!("  [1] Anthropic (Claude)");
    println!("  [2] OpenAI (GPT)");
    println!("  [3] OpenRouter (multiple models)");
    println!();
    print!("Provider [1]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    let provider = match input {
        "" | "1" => "anthropic",
        "2" => "openai",
        "3" => "openrouter",
        _ => {
            println!("{}: Invalid selection '{}'", "Warning".yellow(), input);
            return Ok(None);
        }
    };

    Ok(Some(provider.to_string()))
}

/// Prompts for an API key with hidden input.
///
/// Uses `rpassword` to prevent the key from being displayed on screen.
/// Returns the entered key, which may be empty if the user presses Enter
/// without typing anything.
fn prompt_api_key(provider: &str) -> Result<String> {
    let display_name = capitalize_provider(provider);
    print!("API key for {} (hidden input): ", display_name);
    io::stdout().flush()?;

    let key = rpassword::read_password().context("Failed to read API key")?;

    if !key.is_empty() {
        let masked = if key.len() > 4 {
            format!("...{}", &key[key.len() - 4..])
        } else {
            "****".to_string()
        };
        println!("  Key received: {}", masked.dimmed());
    }

    Ok(key)
}

/// Returns a human-readable capitalized name for the given provider.
fn capitalize_provider(provider: &str) -> &str {
    match provider {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "openrouter" => "OpenRouter",
        _ => provider,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_tools_returns_vec() {
        let registry = default_registry();
        let detected = detect_tools(&registry);

        // Should return a vector (may be empty if no tools installed)
        // We just verify the function runs without error
        let _ = detected.len();
    }

    #[test]
    fn test_detected_tool_has_required_fields() {
        let tool = DetectedTool {
            name: "test-tool".to_string(),
            description: "A test tool".to_string(),
            has_sessions: true,
            session_count: 5,
        };

        assert_eq!(tool.name, "test-tool");
        assert_eq!(tool.description, "A test tool");
        assert!(tool.has_sessions);
        assert_eq!(tool.session_count, 5);
    }

    #[test]
    fn test_cursor_data_path_is_valid() {
        let path = cursor_data_path();
        // Should return a path that contains "Cursor" somewhere
        assert!(path.to_string_lossy().contains("Cursor"));
    }

    #[test]
    fn test_detect_os_returns_expected_type() {
        let os = detect_os();
        // Verify we get the expected OS type for the current platform
        #[cfg(target_os = "macos")]
        assert_eq!(
            os,
            OperatingSystem::MacOS,
            "Expected MacOS on macOS platform"
        );

        #[cfg(target_os = "linux")]
        assert_eq!(
            os,
            OperatingSystem::Linux,
            "Expected Linux on Linux platform"
        );

        #[cfg(target_os = "windows")]
        assert_eq!(
            os,
            OperatingSystem::Windows,
            "Expected Windows on Windows platform"
        );

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        assert_eq!(
            os,
            OperatingSystem::Unknown,
            "Expected Unknown on unsupported platform"
        );
    }

    #[test]
    fn test_operating_system_display() {
        assert_eq!(format!("{}", OperatingSystem::MacOS), "macOS");
        assert_eq!(format!("{}", OperatingSystem::Linux), "Linux");
        assert_eq!(format!("{}", OperatingSystem::Windows), "Windows");
        assert_eq!(format!("{}", OperatingSystem::Unknown), "Unknown");
    }

    #[test]
    fn test_operating_system_equality() {
        assert_eq!(OperatingSystem::MacOS, OperatingSystem::MacOS);
        assert_eq!(OperatingSystem::Linux, OperatingSystem::Linux);
        assert_ne!(OperatingSystem::MacOS, OperatingSystem::Linux);
        assert_ne!(OperatingSystem::Windows, OperatingSystem::Unknown);
    }

    #[test]
    fn test_operating_system_clone() {
        let os = OperatingSystem::MacOS;
        let cloned = os;
        assert_eq!(os, cloned);
    }
}

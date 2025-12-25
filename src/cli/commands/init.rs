//! Init command - guided first-run setup for Lore.
//!
//! Detects installed AI coding tools and creates initial configuration.

use anyhow::{Context, Result};
use colored::Colorize;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::capture::watchers::{default_registry, WatcherRegistry};
use crate::cli::commands::import;
use crate::config::Config;
use crate::storage::db::default_db_path;

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
        println!(
            "Use {} to reconfigure.",
            "lore init --force".cyan()
        );
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
        println!("Install one of these tools and run {} again.", "lore init".cyan());
        return Ok(());
    }

    // Show detected tools
    let tools_with_sessions: Vec<&DetectedTool> = detected
        .iter()
        .filter(|t| t.has_sessions)
        .collect();
    let tools_without_sessions: Vec<&DetectedTool> = detected
        .iter()
        .filter(|t| !t.has_sessions)
        .collect();

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
            println!(
                "    {} - {}",
                tool.name.cyan(),
                tool.description.dimmed()
            );
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

    let config = Config {
        watchers: selected_watchers.clone(),
        ..Config::default()
    };

    // Ensure the config directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    config
        .save_to_path(&config_path)
        .context("Failed to save configuration")?;

    println!("  Created: {}", config_path.display());

    // Initialize database if needed
    if !db_path.exists() {
        // Opening the database will create it and run migrations
        crate::storage::Database::open_default()?;
        println!("  Created: {}", db_path.display());
    }

    println!();
    println!("{}", "Setup complete!".green().bold());
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
                println!(
                    "  ({} skipped, {} errors)",
                    stats.skipped, stats.errors
                );
            }
        }
    }

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

    println!(
        "{}",
        "Which tools would you like to enable?".bold()
    );
    println!();

    // Show numbered options
    for (i, tool) in detected.iter().enumerate() {
        let num = i + 1;
        let status = if tool.has_sessions {
            format!("({} sessions)", tool.session_count).green().to_string()
        } else {
            "(no sessions yet)".dimmed().to_string()
        };
        println!("  [{}] {} - {} {}", num, tool.name.cyan(), tool.description, status);
    }

    println!();
    println!(
        "Enter tool numbers separated by commas, or press Enter to enable all:"
    );
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
                println!(
                    "{}: {} is not a valid option",
                    "Warning".yellow(),
                    num
                );
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
}

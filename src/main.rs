use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::{self, IsTerminal, Write};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod capture;
mod cli;
mod config;
mod daemon;
mod git;
mod storage;

use cli::commands;
use config::Config;

/// The main CLI command line interface.
#[derive(Parser)]
#[command(name = "lore")]
#[command(version)]
#[command(about = "Reasoning history for code - capture the story behind your commits")]
#[command(
    long_about = "Lore captures AI coding sessions and links them to git commits,\n\
    preserving the reasoning behind code changes.\n\n\
    Git captures code history (what changed). Lore captures reasoning\n\
    history (how and why it changed through human-AI collaboration)."
)]
#[command(after_help = "EXAMPLES:\n    \
    lore import              Import sessions from Claude Code\n    \
    lore sessions            List recent sessions\n    \
    lore show abc123         View session details\n    \
    lore show --commit HEAD  View sessions linked to HEAD\n    \
    lore link abc123         Link session to HEAD\n    \
    lore search \"auth\"       Search sessions for text\n    \
    lore daemon start        Start background watcher\n\n\
    For more information about a command, run 'lore <command> --help'.")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output for debugging
    #[arg(short, long, global = true)]
    verbose: bool,
}

/// Available CLI subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Initialize Lore with guided setup
    #[command(
        long_about = "Runs a guided setup wizard that detects installed AI coding tools\n\
        and creates an initial configuration file. Use this when first\n\
        installing Lore or to reconfigure your setup."
    )]
    Init(commands::init::Args),

    /// Show Lore status, database stats, and recent sessions
    #[command(
        long_about = "Displays an overview of the Lore database including session counts,\n\
        watcher availability, daemon status, links to the current commit,\n\
        and a list of recent sessions."
    )]
    Status(commands::status::Args),

    /// List and filter imported sessions
    #[command(
        long_about = "Displays a table of imported sessions with their IDs, timestamps,\n\
        message counts, branches, and directories. Sessions can be filtered\n\
        by repository path."
    )]
    Sessions(commands::sessions::Args),

    /// Show session details or sessions linked to a commit
    #[command(
        long_about = "Displays the full conversation history for a session, or lists\n\
        all sessions linked to a specific commit when using --commit.\n\
        \n\
        Supports multiple output formats:\n\
        - text: colored terminal output (default)\n\
        - json: machine-readable structured output\n\
        - markdown: formatted for documentation"
    )]
    Show(commands::show::Args),

    /// Link sessions to git commits
    #[command(
        long_about = "Creates associations between AI coding sessions and git commits.\n\
        Links can be created manually by specifying session IDs, or\n\
        automatically using --auto to find sessions by time proximity\n\
        and file overlap."
    )]
    Link(commands::link::Args),

    /// Remove session-to-commit links
    #[command(
        long_about = "Removes links between sessions and commits. Can remove a specific\n\
        link using --commit, or remove all links for a session."
    )]
    Unlink(commands::unlink::Args),

    /// Search session content using full-text search
    #[command(
        long_about = "Searches message content using SQLite FTS5 full-text search.\n\
        Supports filtering by repository, date range, and message role.\n\
        The search index is built automatically on first use."
    )]
    Search(commands::search::Args),

    /// View and manage configuration settings
    #[command(
        long_about = "Provides subcommands to show, get, and set configuration values.\n\
        Configuration is stored in ~/.lore/config.yaml."
    )]
    Config(commands::config::Args),

    /// Import sessions from AI coding tools
    #[command(
        long_about = "Discovers and imports session files from AI coding tools into\n\
        the Lore database. Tracks imported files to avoid duplicates.\n\n\
        Supported tools:\n  \
        - Aider (markdown chat history files)\n  \
        - Claude Code (JSONL files in ~/.claude/projects/)\n  \
        - Cline (VS Code extension storage)\n  \
        - Codex CLI (JSONL files in ~/.codex/sessions/)\n  \
        - Continue.dev (JSON files in ~/.continue/sessions/)\n  \
        - Cursor IDE (SQLite databases, experimental)\n  \
        - Gemini CLI (JSON files in ~/.gemini/tmp/)"
    )]
    Import(commands::import::Args),

    /// Manage git hooks for automatic session linking
    #[command(
        long_about = "Installs, uninstalls, or checks the status of git hooks that\n\
        integrate Lore with your git workflow. The post-commit hook\n\
        automatically links sessions to commits."
    )]
    Hooks(commands::hooks::Args),

    /// Manage the background daemon for automatic session capture
    #[command(
        long_about = "Controls the background daemon that watches for new AI coding\n\
        sessions and automatically imports them into the database."
    )]
    Daemon(commands::daemon::Args),
}

/// Checks if Lore is configured (config file exists).
fn is_configured() -> bool {
    Config::config_path()
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// Checks if the given command should skip the first-run prompt.
///
/// Commands that should skip:
/// - `init` (the setup command itself)
/// - `config` (should work without init for debugging)
fn should_skip_first_run_prompt(command: &Commands) -> bool {
    matches!(command, Commands::Init(_) | Commands::Config(_))
}

/// Checks if stdin is connected to a terminal (interactive mode).
fn is_interactive() -> bool {
    io::stdin().is_terminal()
}

/// Prompts the user to run the init wizard.
///
/// Returns `true` if the user wants to run init, `false` otherwise.
fn prompt_for_init() -> Result<bool> {
    print!("Lore isn't configured yet. Run setup? [Y/n] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    // Empty input or "y" or "yes" means yes (default)
    Ok(input.is_empty() || input == "y" || input == "yes")
}

/// Creates a minimal config so the first-run prompt is not shown again.
fn create_minimal_config() -> Result<()> {
    let config = Config::default();
    config.save()
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        "lore=debug"
    } else {
        "lore=info"
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .with(tracing_subscriber::fmt::layer().without_time())
        .init();

    // First-run detection: prompt to run init if not configured
    if !is_configured() && !should_skip_first_run_prompt(&cli.command) && is_interactive() {
        if prompt_for_init()? {
            // Run the init wizard
            commands::init::run(commands::init::Args { force: false })?;
            println!();
            println!("Continuing with your command...");
            println!();
        } else {
            // User declined; create minimal config so we don't ask again
            create_minimal_config()?;
        }
    }

    match cli.command {
        Commands::Init(args) => commands::init::run(args),
        Commands::Status(args) => commands::status::run(args),
        Commands::Sessions(args) => commands::sessions::run(args),
        Commands::Show(args) => commands::show::run(args),
        Commands::Link(args) => commands::link::run(args),
        Commands::Unlink(args) => commands::unlink::run(args),
        Commands::Search(args) => commands::search::run(args),
        Commands::Config(args) => commands::config::run(args),
        Commands::Import(args) => commands::import::run(args),
        Commands::Hooks(args) => commands::hooks::run(args),
        Commands::Daemon(args) => commands::daemon::run(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OutputFormat;

    #[test]
    fn test_should_skip_first_run_prompt_init() {
        let command = Commands::Init(commands::init::Args { force: false });
        assert!(should_skip_first_run_prompt(&command));
    }

    #[test]
    fn test_should_skip_first_run_prompt_init_force() {
        let command = Commands::Init(commands::init::Args { force: true });
        assert!(should_skip_first_run_prompt(&command));
    }

    #[test]
    fn test_should_skip_first_run_prompt_config() {
        let command = Commands::Config(commands::config::Args {
            command: None,
            format: OutputFormat::Text,
        });
        assert!(should_skip_first_run_prompt(&command));
    }

    #[test]
    fn test_should_not_skip_first_run_prompt_status() {
        let command = Commands::Status(commands::status::Args {
            format: OutputFormat::Text,
        });
        assert!(!should_skip_first_run_prompt(&command));
    }

    #[test]
    fn test_should_not_skip_first_run_prompt_sessions() {
        let command = Commands::Sessions(commands::sessions::Args {
            repo: None,
            limit: 20,
            format: OutputFormat::Text,
        });
        assert!(!should_skip_first_run_prompt(&command));
    }

    #[test]
    fn test_should_not_skip_first_run_prompt_import() {
        let command = Commands::Import(commands::import::Args {
            force: false,
            dry_run: false,
        });
        assert!(!should_skip_first_run_prompt(&command));
    }
}

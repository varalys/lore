use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Reset SIGPIPE to default behavior on Unix systems.
// This prevents panics when piping output to commands like `head` that close
// the pipe early. Without this, writing to a closed pipe causes EPIPE errors
// which manifest as panics in libraries that don't handle them gracefully.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {
    // No-op on non-Unix systems
}

mod capture;
mod cli;
mod config;
mod daemon;
mod git;
mod storage;

use cli::commands;
use config::Config;

// ANSI escape codes for terminal colors
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// Timeout duration for the init prompt in seconds.
const PROMPT_TIMEOUT_SECS: u64 = 30;

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
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output for debugging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Skip the first-run setup prompt (useful for scripting)
    #[arg(long, global = true)]
    no_init: bool,
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

    /// Permanently delete a session and its data
    #[command(
        long_about = "Permanently removes a session and all its associated data\n\
        (messages and links) from the database. This operation cannot\n\
        be undone."
    )]
    Delete(commands::delete::Args),

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

    /// Manage the database (vacuum, prune, stats)
    #[command(
        long_about = "Database management commands for maintenance and statistics.\n\
        Includes vacuum (reclaim space), prune (delete old sessions),\n\
        and stats (show database statistics)."
    )]
    Db(commands::db::Args),

    /// Generate shell completions
    #[command(
        long_about = "Generates shell completion scripts for various shells.\n\
        Output to stdout for redirection to the appropriate file."
    )]
    Completions(commands::completions::Args),
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
/// - `completions` (should work without init for shell setup)
fn should_skip_first_run_prompt(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Init(_) | Commands::Config(_) | Commands::Completions(_)
    )
}

/// Checks if stdin is connected to a terminal (interactive mode).
fn is_interactive() -> bool {
    io::stdin().is_terminal()
}

/// Checks if stdout is connected to a terminal (for color output).
fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

/// Result of the init prompt, including timeout case.
#[derive(Debug, PartialEq)]
enum PromptResult {
    /// User chose to run init
    Yes,
    /// User declined init
    No,
    /// Prompt timed out with no response
    Timeout,
}

/// Prompts the user to run the init wizard with colored output and timeout.
///
/// Returns the user's choice or timeout after 30 seconds.
/// Uses ANSI colors if stdout is a TTY.
fn prompt_for_init() -> Result<PromptResult> {
    let use_color = stdout_is_tty();

    // Build the prompt with optional colors
    if use_color {
        print!(
            "{BOLD}{YELLOW}Lore isn't configured yet. Run setup?{RESET} [{GREEN}Y{RESET}/{RED}n{RESET}] "
        );
    } else {
        print!("Lore isn't configured yet. Run setup? [Y/n] ");
    }
    io::stdout().flush()?;

    // Use a channel to receive input with timeout
    let (tx, rx) = mpsc::channel();

    // Spawn a thread to read input
    thread::spawn(move || {
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            let _ = tx.send(input);
        }
    });

    // Wait for input with timeout
    match rx.recv_timeout(Duration::from_secs(PROMPT_TIMEOUT_SECS)) {
        Ok(input) => {
            let input = input.trim().to_lowercase();
            // Empty input or "y" or "yes" means yes (default)
            if input.is_empty() || input == "y" || input == "yes" {
                Ok(PromptResult::Yes)
            } else {
                Ok(PromptResult::No)
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            println!();
            Ok(PromptResult::Timeout)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Sender dropped without sending, treat as no
            println!();
            Ok(PromptResult::No)
        }
    }
}

/// Creates a minimal config so the first-run prompt is not shown again.
fn create_minimal_config() -> Result<()> {
    let config = Config::default();
    config.save()
}

/// Returns a user-friendly name for the command being executed.
fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Init(_) => "init",
        Commands::Status(_) => "status",
        Commands::Sessions(_) => "sessions",
        Commands::Show(_) => "show",
        Commands::Link(_) => "link",
        Commands::Unlink(_) => "unlink",
        Commands::Delete(_) => "delete",
        Commands::Search(_) => "search",
        Commands::Config(_) => "config",
        Commands::Import(_) => "import",
        Commands::Hooks(_) => "hooks",
        Commands::Daemon(_) => "daemon",
        Commands::Db(_) => "db",
        Commands::Completions(_) => "completions",
    }
}

fn main() -> Result<()> {
    // Handle SIGPIPE to prevent panics when piping to commands like `head`
    reset_sigpipe();

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
    // Skip if --no-init flag is set (useful for scripting)
    if !cli.no_init
        && !is_configured()
        && !should_skip_first_run_prompt(&cli.command)
        && is_interactive()
    {
        match prompt_for_init()? {
            PromptResult::Yes => {
                // Run the init wizard with force=true since user explicitly chose setup
                commands::init::run(commands::init::Args { force: true })?;
                println!();
                println!(
                    "Setup complete! Running 'lore {}'...",
                    command_name(&cli.command)
                );
                println!();
            }
            PromptResult::No => {
                // User declined; create minimal config so we don't ask again
                println!("Okay, run 'lore init' anytime to configure.");
                println!();
                create_minimal_config()?;
            }
            PromptResult::Timeout => {
                // Timed out; create minimal config so we don't hang again
                println!("No response, continuing without setup...");
                println!();
                create_minimal_config()?;
            }
        }
    }

    match cli.command {
        Commands::Init(args) => commands::init::run(args),
        Commands::Status(args) => commands::status::run(args),
        Commands::Sessions(args) => commands::sessions::run(args),
        Commands::Show(args) => commands::show::run(args),
        Commands::Link(args) => commands::link::run(args),
        Commands::Unlink(args) => commands::unlink::run(args),
        Commands::Delete(args) => commands::delete::run(args),
        Commands::Search(args) => commands::search::run(args),
        Commands::Config(args) => commands::config::run(args),
        Commands::Import(args) => commands::import::run(args),
        Commands::Hooks(args) => commands::hooks::run(args),
        Commands::Daemon(args) => commands::daemon::run(args),
        Commands::Db(args) => commands::db::run(args),
        Commands::Completions(args) => {
            let mut cmd = Cli::command();
            commands::completions::generate_completions(&mut cmd, args.shell);
            Ok(())
        }
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

    #[test]
    fn test_cli_no_init_flag_parses() {
        use clap::Parser;
        // Test that --no-init flag is recognized and parsed correctly
        let cli = Cli::try_parse_from(["lore", "--no-init", "status"]);
        assert!(cli.is_ok());
        let cli = cli.unwrap();
        assert!(cli.no_init);
    }

    #[test]
    fn test_cli_no_init_flag_default_false() {
        use clap::Parser;
        // Test that --no-init defaults to false when not provided
        let cli = Cli::try_parse_from(["lore", "status"]);
        assert!(cli.is_ok());
        let cli = cli.unwrap();
        assert!(!cli.no_init);
    }

    #[test]
    fn test_cli_no_init_flag_with_verbose() {
        use clap::Parser;
        // Test that --no-init and --verbose can be combined
        let cli = Cli::try_parse_from(["lore", "--no-init", "--verbose", "sessions"]);
        assert!(cli.is_ok());
        let cli = cli.unwrap();
        assert!(cli.no_init);
        assert!(cli.verbose);
    }

    #[test]
    fn test_command_name_status() {
        let command = Commands::Status(commands::status::Args {
            format: OutputFormat::Text,
        });
        assert_eq!(command_name(&command), "status");
    }

    #[test]
    fn test_command_name_sessions() {
        let command = Commands::Sessions(commands::sessions::Args {
            repo: None,
            limit: 20,
            format: OutputFormat::Text,
        });
        assert_eq!(command_name(&command), "sessions");
    }

    #[test]
    fn test_command_name_import() {
        let command = Commands::Import(commands::import::Args {
            force: false,
            dry_run: false,
        });
        assert_eq!(command_name(&command), "import");
    }

    #[test]
    fn test_command_name_init() {
        let command = Commands::Init(commands::init::Args { force: false });
        assert_eq!(command_name(&command), "init");
    }

    #[test]
    fn test_prompt_result_equality() {
        // Test that PromptResult enum variants are distinguishable
        assert_eq!(PromptResult::Yes, PromptResult::Yes);
        assert_eq!(PromptResult::No, PromptResult::No);
        assert_eq!(PromptResult::Timeout, PromptResult::Timeout);
        assert_ne!(PromptResult::Yes, PromptResult::No);
        assert_ne!(PromptResult::Yes, PromptResult::Timeout);
        assert_ne!(PromptResult::No, PromptResult::Timeout);
    }

    #[test]
    fn test_completions_bash() {
        use clap_complete::{generate, Shell};
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Bash, &mut cmd, "lore", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(
            output.contains("_lore"),
            "Should contain bash completion function"
        );
    }

    #[test]
    fn test_completions_zsh() {
        use clap_complete::{generate, Shell};
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Zsh, &mut cmd, "lore", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(
            output.contains("#compdef lore"),
            "Should contain zsh compdef"
        );
    }

    #[test]
    fn test_completions_fish() {
        use clap_complete::{generate, Shell};
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Fish, &mut cmd, "lore", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(
            output.contains("complete -c lore"),
            "Should contain fish completion"
        );
    }

    #[test]
    fn test_completions_powershell() {
        use clap_complete::{generate, Shell};
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(Shell::PowerShell, &mut cmd, "lore", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(
            output.contains("Register-ArgumentCompleter"),
            "Should contain powershell completer"
        );
    }

    #[test]
    fn test_completions_elvish() {
        use clap_complete::{generate, Shell};
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Elvish, &mut cmd, "lore", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(
            output.contains("set edit:completion"),
            "Should contain elvish completion"
        );
    }
}

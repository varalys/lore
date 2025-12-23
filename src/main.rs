use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cli;
mod capture;
mod config;
mod daemon;
mod git;
mod storage;

use cli::commands;

/// The main CLI command line interface.
#[derive(Parser)]
#[command(name = "lore")]
#[command(version)]
#[command(about = "Reasoning history for code - capture the story behind your commits")]
#[command(long_about = "Lore captures AI coding sessions and links them to git commits,\n\
    preserving the reasoning behind code changes.\n\n\
    Git captures code history (what changed). Lore captures reasoning\n\
    history (how and why it changed through human-AI collaboration).")]
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
    /// Show Lore status, database stats, and recent sessions
    #[command(long_about = "Displays an overview of the Lore database including session counts,\n\
        watcher availability, daemon status, links to the current commit,\n\
        and a list of recent sessions.")]
    Status(commands::status::Args),

    /// List and filter imported sessions
    #[command(long_about = "Displays a table of imported sessions with their IDs, timestamps,\n\
        message counts, branches, and directories. Sessions can be filtered\n\
        by repository path.")]
    Sessions(commands::sessions::Args),

    /// Show session details or sessions linked to a commit
    #[command(long_about = "Displays the full conversation history for a session, or lists\n\
        all sessions linked to a specific commit when using --commit.\n\
        \n\
        Supports multiple output formats:\n\
        - text: colored terminal output (default)\n\
        - json: machine-readable structured output\n\
        - markdown: formatted for documentation")]
    Show(commands::show::Args),

    /// Link sessions to git commits
    #[command(long_about = "Creates associations between AI coding sessions and git commits.\n\
        Links can be created manually by specifying session IDs, or\n\
        automatically using --auto to find sessions by time proximity\n\
        and file overlap.")]
    Link(commands::link::Args),

    /// Remove session-to-commit links
    #[command(long_about = "Removes links between sessions and commits. Can remove a specific\n\
        link using --commit, or remove all links for a session.")]
    Unlink(commands::unlink::Args),

    /// Search session content using full-text search
    #[command(long_about = "Searches message content using SQLite FTS5 full-text search.\n\
        Supports filtering by repository, date range, and message role.\n\
        The search index is built automatically on first use.")]
    Search(commands::search::Args),

    /// View and manage configuration settings
    #[command(long_about = "Provides subcommands to show, get, and set configuration values.\n\
        Configuration is stored in ~/.lore/config.yaml.")]
    Config(commands::config::Args),

    /// Import sessions from AI coding tools
    #[command(long_about = "Discovers and imports session files from AI coding tools into\n\
        the Lore database. Currently supports Claude Code, Cursor, Cline,\n\
        Aider, and Continue. Tracks imported files to avoid duplicates.")]
    Import(commands::import::Args),

    /// Manage git hooks for automatic session linking
    #[command(long_about = "Installs, uninstalls, or checks the status of git hooks that\n\
        integrate Lore with your git workflow. The post-commit hook\n\
        automatically links sessions to commits.")]
    Hooks(commands::hooks::Args),

    /// Manage the background daemon for automatic session capture
    #[command(long_about = "Controls the background daemon that watches for new AI coding\n\
        sessions and automatically imports them into the database.")]
    Daemon(commands::daemon::Args),
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
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()))
        .with(tracing_subscriber::fmt::layer().without_time())
        .init();

    match cli.command {
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

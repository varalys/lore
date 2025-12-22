use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cli;
mod capture;
mod config;
mod git;
mod storage;

use cli::commands;

#[derive(Parser)]
#[command(name = "lore")]
#[command(about = "Reasoning history for code - capture the story behind your commits")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Show current status and recent sessions
    Status,

    /// List and filter sessions
    Sessions(commands::sessions::Args),

    /// Show session details or sessions linked to a commit
    Show(commands::show::Args),

    /// Link sessions to commits
    Link(commands::link::Args),

    /// Unlink sessions from commits
    Unlink(commands::unlink::Args),

    /// Search sessions
    Search(commands::search::Args),

    /// Manage configuration
    Config(commands::config::Args),

    /// Import sessions from Claude Code (temporary command for development)
    Import(commands::import::Args),
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
        Commands::Status => commands::status::run(),
        Commands::Sessions(args) => commands::sessions::run(args),
        Commands::Show(args) => commands::show::run(args),
        Commands::Link(args) => commands::link::run(args),
        Commands::Unlink(args) => commands::unlink::run(args),
        Commands::Search(args) => commands::search::run(args),
        Commands::Config(args) => commands::config::run(args),
        Commands::Import(args) => commands::import::run(args),
    }
}

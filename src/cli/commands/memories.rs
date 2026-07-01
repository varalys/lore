//! Memories command - list a project's mirrored memories.
//!
//! Refreshes the read-only mirror of a coding tool's per-project memory store
//! (currently Claude Code) and lists the current memories for the repository.
//! Lore never writes back to the tool's memory folder; this only reflects it.

use anyhow::Result;
use colored::Colorize;

use crate::capture::memory::{resolve_project_path, MemoryMirror, CLAUDE_CODE_TOOL};
use crate::cli::OutputFormat;
use crate::storage::Database;

/// Arguments for the memories command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore memories                  List memories for the current repository\n    \
    lore memories --project /path  List memories for a specific repository\n    \
    lore memories --format json    Output as JSON")]
pub struct Args {
    /// Repository path to read memories for (defaults to the current repo)
    #[arg(short, long, value_name = "PATH")]
    #[arg(
        long_help = "Read memories for this repository path. Defaults to the\n\
        current directory resolved to its git top-level."
    )]
    pub project: Option<String>,

    /// Output format: text (default), json
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// Executes the memories command.
///
/// Resolves the target repository, refreshes the memory mirror from the tool's
/// memory folder, and lists the current memories.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;
    let project = resolve_project_path(args.project.as_deref())?;

    // Refresh-on-read so results reflect the current folder state.
    let mirror = MemoryMirror::claude();
    mirror.refresh(&db, &project)?;

    let project_key = project.to_string_lossy().to_string();
    let memories = db.get_memories(&project_key, CLAUDE_CODE_TOOL)?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&memories)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            if memories.is_empty() {
                println!("{}", "No memories found for this project.".dimmed());
                println!();
                println!(
                    "Memories are mirrored read-only from Claude Code's memory folder\n\
                     for this repository. None were found at:"
                );
                println!("  {}", mirror.memory_dir(&project).display());
                return Ok(());
            }

            println!(
                "{}",
                format!("Memories for {project_key} (source: {CLAUDE_CODE_TOOL})").bold()
            );
            println!();

            for memory in &memories {
                let type_label = memory
                    .memory_type
                    .as_deref()
                    .map(|t| format!(" [{t}]"))
                    .unwrap_or_default();
                println!("{}{}", memory.name.cyan().bold(), type_label.yellow());
                if let Some(ref desc) = memory.description {
                    println!("  {}", desc.dimmed());
                }
                for line in memory.content.lines() {
                    println!("  {line}");
                }
                println!();
            }
        }
    }

    Ok(())
}

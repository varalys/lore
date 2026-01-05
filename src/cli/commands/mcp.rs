//! MCP server command.
//!
//! Starts the Model Context Protocol server for exposing Lore data to
//! AI coding tools like Claude Code.

use anyhow::Result;

/// Arguments for the mcp command.
#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: McpCommand,
}

/// MCP subcommands.
#[derive(clap::Subcommand)]
pub enum McpCommand {
    /// Start the MCP server on stdio
    #[command(
        long_about = "Starts the MCP (Model Context Protocol) server on stdio.\n\
        The server reads JSON-RPC requests from stdin and writes responses\n\
        to stdout. This allows AI coding tools like Claude Code to query\n\
        Lore session data.\n\n\
        Available tools:\n  \
        - lore_search: Search session messages\n  \
        - lore_get_session: Get full session details\n  \
        - lore_list_sessions: List recent sessions\n  \
        - lore_get_context: Get repository context\n  \
        - lore_get_linked_sessions: Get sessions linked to a commit"
    )]
    Serve,
}

/// Executes the mcp command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        McpCommand::Serve => run_serve(),
    }
}

/// Runs the MCP server.
fn run_serve() -> Result<()> {
    // Create a new tokio runtime for the MCP server
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(crate::mcp::run_server())
}

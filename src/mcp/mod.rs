//! MCP (Model Context Protocol) server for Lore.
//!
//! Exposes Lore session data to AI tools via the Model Context Protocol,
//! allowing Claude Code and other MCP-compatible tools to query session
//! history, search messages, and access linked commits.
//!
//! The server runs on stdio transport and implements the following tools:
//! - `lore_search`: Search sessions by query with filters
//! - `lore_get_session`: Get full session transcript by ID
//! - `lore_list_sessions`: List recent sessions with optional filters
//! - `lore_get_context`: Get recent session context for a repository
//! - `lore_get_linked_sessions`: Get sessions linked to a commit

mod server;
mod tools;

pub use server::run_server;

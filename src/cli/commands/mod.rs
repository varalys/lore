//! CLI commands for Lore.
//!
//! Each submodule implements a single CLI command with its argument
//! parsing and execution logic.

/// Configuration viewing and management.
pub mod config;

/// Import sessions from AI coding tools.
pub mod import;

/// Link sessions to git commits.
pub mod link;

/// Search session content using FTS5 full-text search.
pub mod search;

/// List and filter sessions.
pub mod sessions;

/// Display session details or commit-linked sessions.
pub mod show;

/// Show current Lore status and recent sessions.
pub mod status;

/// Remove session-to-commit links (not yet implemented).
pub mod unlink;

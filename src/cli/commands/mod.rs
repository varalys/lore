//! CLI commands for Lore.
//!
//! Each submodule implements a single CLI command with its argument
//! parsing and execution logic.

/// Add a bookmark or note to a session.
pub mod annotate;

/// Shell completion script generation.
pub mod completions;

/// Configuration viewing and management.
pub mod config;

/// Show recent sessions for quick orientation.
pub mod context;

/// Show the active session for the current directory.
pub mod current;

/// Background daemon management (start, stop, status, logs).
pub mod daemon;

/// Database management (vacuum, prune, stats).
pub mod db;

/// Permanently delete a session and its data.
pub mod delete;

/// Git hooks management (install, uninstall, status).
pub mod hooks;

/// Import sessions from AI coding tools.
pub mod import;

/// Guided first-run setup.
pub mod init;

/// Link sessions to git commits.
pub mod link;

/// Search session content using FTS5 full-text search.
pub mod search;

/// List and filter sessions.
pub mod sessions;

/// Display session details or commit-linked sessions.
pub mod show;

/// Add or view session summaries.
pub mod summarize;

/// Show current Lore status and recent sessions.
pub mod status;

/// Add or remove tags from sessions.
pub mod tag;

/// Remove session-to-commit links.
pub mod unlink;

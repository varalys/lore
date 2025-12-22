//! Storage layer for Lore.
//!
//! This module provides SQLite-based persistence for sessions, messages,
//! and session-to-commit links. It handles schema migrations and provides
//! query methods for the CLI and future daemon.
//!
//! # Submodules
//!
//! - `db` - Database connection and query operations
//! - `models` - Data structures for sessions, messages, and links

/// SQLite database connection and query operations.
pub mod db;

/// Data structures representing sessions, messages, and links.
pub mod models;

pub use db::Database;
pub use models::{
    extract_session_files, ContentBlock, LinkCreator, LinkType, MessageContent, MessageRole,
    SessionLink,
};

// Re-exported for use by integration tests. These types are used through the
// storage module in tests/cli_integration.rs even though they're not directly
// used in the binary crate itself.
#[allow(unused_imports)]
pub use models::{Message, Session};

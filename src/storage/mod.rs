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
// DatabaseStats is also available at crate::storage::db::DatabaseStats if needed
pub use models::{
    extract_session_files, Annotation, ContentBlock, LinkCreator, LinkType, MessageContent,
    MessageRole, SessionLink, Summary, Tag,
};

// Re-exported for use by integration tests. These types are used through the
// storage module in tests/cli_integration.rs even though they're not directly
// used in the binary crate itself.
#[allow(unused_imports)]
pub use models::{Message, Session};

/// Returns the machine identifier (hostname) for the current machine.
///
/// Used to populate the `machine_id` field on sessions, allowing cloud sync
/// to identify which machine created a session. Returns `None` if the hostname
/// cannot be determined.
pub fn get_machine_id() -> Option<String> {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
}

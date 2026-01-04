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

use crate::config::Config;

/// SQLite database connection and query operations.
pub mod db;

/// Data structures representing sessions, messages, and links.
pub mod models;

pub use db::Database;
// DatabaseStats is also available at crate::storage::db::DatabaseStats if needed
pub use models::{
    extract_session_files, Annotation, ContentBlock, LinkCreator, LinkType, Machine,
    MessageContent, MessageRole, SessionLink, Summary, Tag,
};

// Re-exported for use by integration tests. These types are used through the
// storage module in tests/cli_integration.rs even though they're not directly
// used in the binary crate itself.
#[allow(unused_imports)]
pub use models::{Message, Session};

/// Returns the machine UUID for the current machine.
///
/// Loads the config and returns the machine_id (UUID), generating one if needed.
/// Used to populate the `machine_id` field on sessions, allowing cloud sync
/// to identify which machine created a session. Returns `None` if the config
/// cannot be loaded or the machine ID cannot be determined.
pub fn get_machine_id() -> Option<String> {
    Config::load()
        .ok()
        .and_then(|mut config| config.get_or_create_machine_id().ok())
}

/// Returns a display-friendly name for a machine ID.
///
/// First queries the machines table to find a registered name. If not found,
/// falls back to checking if this is the current machine and uses the config.
/// Otherwise returns the machine_id truncated to first 8 characters for readability.
///
/// This function is designed for use in session listings to show human-readable
/// machine names instead of UUIDs.
#[allow(dead_code)]
pub fn get_machine_display_name(db: &Database, machine_id: &str) -> String {
    // First try to get from the machines table
    if let Ok(name) = db.get_machine_name(machine_id) {
        // get_machine_name returns the name if found, or truncated ID if not
        // Check if we got a full name (not just truncated ID)
        if let Ok(Some(_machine)) = db.get_machine(machine_id) {
            return name;
        }
    }

    // Fall back to checking if this is the current machine
    if let Ok(mut config) = Config::load() {
        if let Ok(current_id) = config.get_or_create_machine_id() {
            if machine_id == current_id {
                return config.get_machine_name();
            }
        }
    }

    // Not found anywhere, show truncated UUID
    if machine_id.len() >= 8 {
        format!("{}...", &machine_id[..8])
    } else {
        machine_id.to_string()
    }
}

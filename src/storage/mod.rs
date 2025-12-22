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
pub use models::*;

//! Watchers for different AI coding tools.
//!
//! Each watcher module provides functions to discover and parse session
//! files from a specific AI coding tool. Watchers convert tool-specific
//! formats into Lore's internal session and message models.

/// Claude Code session parser for JSONL files.
pub mod claude_code;


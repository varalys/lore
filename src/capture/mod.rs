//! Session capture from AI coding tools.
//!
//! This module provides parsers for importing sessions from various AI
//! coding assistants. Each tool has its own session format and storage
//! location.
//!
//! # Supported Tools
//!
//! - Claude Code - Parses JSONL files from `~/.claude/projects/`
//!
//! # Future Tools
//!
//! - Cursor - Will parse from Cursor's session storage
//! - GitHub Copilot - Will parse from Copilot's logs

/// Tool-specific session parsers.
pub mod watchers;

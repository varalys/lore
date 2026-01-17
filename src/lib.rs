//! Lore - Reasoning history for code
//!
//! Lore captures the story behind your commits by recording AI-assisted
//! development sessions and linking them to git history.
//!
//! Git captures code history (what changed). Lore captures reasoning history
//! (how and why it changed through human-AI collaboration).
//!
//! # Modules
//!
//! - [`capture`] - Session capture from AI coding tools
//! - [`cloud`] - Cloud sync for cross-machine session access
//! - [`config`] - Configuration management
//! - [`daemon`] - Background daemon for automatic session capture
//! - [`git`] - Git repository integration and auto-linking
//! - [`mcp`] - MCP (Model Context Protocol) server
//! - [`storage`] - SQLite database operations and data models

/// Session capture from AI coding tools like Claude Code and Copilot.
pub mod capture;

/// Cloud sync for cross-machine session access.
pub mod cloud;

/// Configuration management for Lore settings.
pub mod config;

/// Background daemon for automatic session capture and file watching.
pub mod daemon;

/// Git repository integration for commit linking and auto-detection.
pub mod git;

/// MCP (Model Context Protocol) server for exposing Lore data to AI tools.
pub mod mcp;

/// SQLite storage layer for sessions, messages, and links.
pub mod storage;

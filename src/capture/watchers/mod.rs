//! Watchers for different AI coding tools.
//!
//! Each watcher module provides functions to discover and parse session
//! files from a specific AI coding tool. Watchers convert tool-specific
//! formats into Lore's internal session and message models.
//!
//! The [`Watcher`] trait defines the common interface for all tool watchers.
//! Use the [`WatcherRegistry`] to manage multiple watchers and query their
//! availability.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::storage::models::{Message, Session};

/// Aider session parser for markdown chat history files.
pub mod aider;

/// Claude Code session parser for JSONL files.
pub mod claude_code;

/// Cline (Claude Dev) session parser for VS Code extension storage.
pub mod cline;

/// Codex CLI session parser for JSONL files.
pub mod codex;

/// Continue.dev session parser for JSON session files.
pub mod continue_dev;

/// Cursor IDE session parser for SQLite databases (experimental).
pub mod cursor;

/// Gemini CLI session parser for JSON files.
pub mod gemini;

/// Information about a tool that can be watched for sessions.
///
/// Contains metadata about the watcher including its name, description,
/// and default file system paths to search for sessions.
#[derive(Debug, Clone)]
pub struct WatcherInfo {
    /// Short identifier for the watcher (e.g., "claude-code", "cursor").
    pub name: &'static str,

    /// Human-readable description of what this watcher handles.
    #[allow(dead_code)]
    pub description: &'static str,

    /// Default file system paths where this tool stores sessions.
    #[allow(dead_code)]
    pub default_paths: Vec<PathBuf>,
}

/// A watcher for AI tool sessions.
///
/// Implementations of this trait can discover and parse session files from
/// a specific AI coding tool. The trait is object-safe to allow storing
/// multiple watcher implementations in a registry.
///
/// # Example
///
/// ```no_run
/// use lore::capture::watchers::default_registry;
///
/// let registry = default_registry();
/// for watcher in registry.available_watchers() {
///     println!("{}: {}", watcher.info().name, watcher.info().description);
/// }
/// ```
pub trait Watcher: Send + Sync {
    /// Returns information about this watcher.
    fn info(&self) -> WatcherInfo;

    /// Checks if this watcher is available.
    ///
    /// A watcher is available if the tool it watches is installed and its
    /// session storage location exists on this system.
    fn is_available(&self) -> bool;

    /// Finds all session sources (files or directories) to import.
    ///
    /// Returns paths to individual session files or databases that can be
    /// passed to [`parse_source`](Self::parse_source).
    fn find_sources(&self) -> Result<Vec<PathBuf>>;

    /// Parses a session source and returns sessions with their messages.
    ///
    /// Each session is returned with its associated messages as a tuple.
    /// A single source file may contain multiple sessions.
    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>>;

    /// Returns paths to watch for changes.
    ///
    /// Used by the daemon file watcher to monitor for new or modified sessions.
    fn watch_paths(&self) -> Vec<PathBuf>;
}

/// Registry of available session watchers.
///
/// The registry maintains a collection of watcher implementations and
/// provides methods to query their availability and retrieve watchers by name.
pub struct WatcherRegistry {
    watchers: Vec<Box<dyn Watcher>>,
}

impl Default for WatcherRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WatcherRegistry {
    /// Creates an empty watcher registry.
    pub fn new() -> Self {
        Self {
            watchers: Vec::new(),
        }
    }

    /// Registers a new watcher with the registry.
    pub fn register(&mut self, watcher: Box<dyn Watcher>) {
        self.watchers.push(watcher);
    }

    /// Returns all registered watchers.
    pub fn all_watchers(&self) -> Vec<&dyn Watcher> {
        self.watchers.iter().map(|w| w.as_ref()).collect()
    }

    /// Returns only watchers that are currently available.
    ///
    /// A watcher is available if the tool it watches is installed
    /// and configured on this system.
    pub fn available_watchers(&self) -> Vec<&dyn Watcher> {
        self.watchers
            .iter()
            .filter(|w| w.is_available())
            .map(|w| w.as_ref())
            .collect()
    }

    /// Retrieves a watcher by its name.
    ///
    /// Returns `None` if no watcher with the given name is registered.
    #[allow(dead_code)]
    pub fn get_watcher(&self, name: &str) -> Option<&dyn Watcher> {
        self.watchers
            .iter()
            .find(|w| w.info().name == name)
            .map(|w| w.as_ref())
    }

    /// Returns all paths that should be watched for changes.
    ///
    /// Collects watch paths from all available watchers into a single list.
    pub fn all_watch_paths(&self) -> Vec<PathBuf> {
        self.available_watchers()
            .iter()
            .flat_map(|w| w.watch_paths())
            .collect()
    }
}

/// Creates the default registry with all built-in watchers.
///
/// This includes watchers for:
/// - Aider (markdown files in project directories)
/// - Claude Code (JSONL files in ~/.claude/projects/)
/// - Cline (JSON files in VS Code extension storage)
/// - Codex CLI (JSONL files in ~/.codex/sessions/)
/// - Continue.dev (JSON files in ~/.continue/sessions/)
/// - Cursor IDE (SQLite databases in workspace storage, experimental)
/// - Gemini CLI (JSON files in ~/.gemini/tmp/)
pub fn default_registry() -> WatcherRegistry {
    let mut registry = WatcherRegistry::new();
    registry.register(Box::new(aider::AiderWatcher));
    registry.register(Box::new(claude_code::ClaudeCodeWatcher));
    registry.register(Box::new(cline::ClineWatcher));
    registry.register(Box::new(codex::CodexWatcher));
    registry.register(Box::new(continue_dev::ContinueDevWatcher));
    registry.register(Box::new(cursor::CursorWatcher));
    registry.register(Box::new(gemini::GeminiWatcher));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test watcher implementation for unit testing the registry.
    struct TestWatcher {
        name: &'static str,
        available: bool,
    }

    impl Watcher for TestWatcher {
        fn info(&self) -> WatcherInfo {
            WatcherInfo {
                name: self.name,
                description: "Test watcher",
                default_paths: vec![PathBuf::from("/test")],
            }
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn find_sources(&self) -> Result<Vec<PathBuf>> {
            Ok(vec![])
        }

        fn parse_source(&self, _path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
            Ok(vec![])
        }

        fn watch_paths(&self) -> Vec<PathBuf> {
            vec![PathBuf::from("/test")]
        }
    }

    #[test]
    fn test_registry_new_is_empty() {
        let registry = WatcherRegistry::new();
        assert!(registry.all_watchers().is_empty());
    }

    #[test]
    fn test_registry_register_and_retrieve() {
        let mut registry = WatcherRegistry::new();
        registry.register(Box::new(TestWatcher {
            name: "test-watcher",
            available: true,
        }));

        assert_eq!(registry.all_watchers().len(), 1);
        assert!(registry.get_watcher("test-watcher").is_some());
        assert!(registry.get_watcher("nonexistent").is_none());
    }

    #[test]
    fn test_registry_available_watchers_filters() {
        let mut registry = WatcherRegistry::new();
        registry.register(Box::new(TestWatcher {
            name: "available",
            available: true,
        }));
        registry.register(Box::new(TestWatcher {
            name: "unavailable",
            available: false,
        }));

        assert_eq!(registry.all_watchers().len(), 2);
        assert_eq!(registry.available_watchers().len(), 1);
        assert_eq!(registry.available_watchers()[0].info().name, "available");
    }

    #[test]
    fn test_registry_all_watch_paths() {
        let mut registry = WatcherRegistry::new();
        registry.register(Box::new(TestWatcher {
            name: "watcher1",
            available: true,
        }));
        registry.register(Box::new(TestWatcher {
            name: "watcher2",
            available: true,
        }));
        registry.register(Box::new(TestWatcher {
            name: "watcher3",
            available: false,
        }));

        let paths = registry.all_watch_paths();
        // Only available watchers contribute paths
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_default_registry_contains_builtin_watchers() {
        let registry = default_registry();
        let watchers = registry.all_watchers();

        // Should have all built-in watchers
        assert!(watchers.len() >= 7);

        // Check that all watchers are registered
        assert!(registry.get_watcher("aider").is_some());
        assert!(registry.get_watcher("claude-code").is_some());
        assert!(registry.get_watcher("cline").is_some());
        assert!(registry.get_watcher("codex").is_some());
        assert!(registry.get_watcher("continue").is_some());
        assert!(registry.get_watcher("cursor").is_some());
        assert!(registry.get_watcher("gemini").is_some());
    }

    #[test]
    fn test_watcher_info_fields() {
        let watcher = TestWatcher {
            name: "test",
            available: true,
        };
        let info = watcher.info();

        assert_eq!(info.name, "test");
        assert_eq!(info.description, "Test watcher");
        assert!(!info.default_paths.is_empty());
    }
}

//! Shared test infrastructure for watcher implementations.
//!
//! This module provides shared tests for common Watcher trait behavior across
//! all watcher implementations. These tests use the default_registry to verify
//! that all registered watchers meet the expected interface contract.
//!
//! Tool-specific parsing tests (testing unique file formats, edge cases) remain
//! in their respective watcher modules.
//!
//! The shared tests reduce duplication while ensuring all watchers maintain
//! consistent behavior for the Watcher trait interface.

use super::*;

/// Tests that all watchers in the default registry have valid info.
///
/// Verifies:
/// - name is non-empty
/// - description is non-empty
#[test]
fn test_all_watchers_have_valid_info() {
    let registry = default_registry();
    let watchers = registry.all_watchers();

    assert!(
        !watchers.is_empty(),
        "Default registry should have watchers"
    );

    for watcher in watchers {
        let info = watcher.info();
        assert!(!info.name.is_empty(), "Watcher name should not be empty");
        assert!(
            !info.description.is_empty(),
            "Watcher {} description should not be empty",
            info.name
        );
    }
}

/// Tests that all watchers have unique names.
#[test]
fn test_all_watchers_have_unique_names() {
    let registry = default_registry();
    let watchers = registry.all_watchers();

    let mut names: Vec<&str> = watchers.iter().map(|w| w.info().name).collect();
    let original_count = names.len();
    names.sort();
    names.dedup();

    assert_eq!(
        names.len(),
        original_count,
        "Watcher names should be unique"
    );
}

/// Tests that find_sources does not error even when directories do not exist.
///
/// This is important for graceful handling on systems where not all AI tools
/// are installed.
#[test]
fn test_all_watchers_find_sources_handles_missing_dirs() {
    let registry = default_registry();
    let watchers = registry.all_watchers();

    for watcher in watchers {
        let info = watcher.info();
        let result = watcher.find_sources();

        assert!(
            result.is_ok(),
            "Watcher {} find_sources should not error when directory is missing: {:?}",
            info.name,
            result.err()
        );
    }
}

/// Tests that watch_paths returns valid paths for watchers that support watching.
///
/// Some watchers (like aider) return empty watch_paths because their files
/// are scattered across project directories, making real-time watching
/// impractical.
#[test]
fn test_all_watchers_watch_paths_are_valid() {
    let registry = default_registry();
    let watchers = registry.all_watchers();

    for watcher in watchers {
        let info = watcher.info();
        let paths = watcher.watch_paths();

        // Paths are valid if either:
        // 1. Empty (watcher doesn't support real-time watching)
        // 2. Contains valid PathBuf entries
        for path in &paths {
            // PathBuf should not be empty strings
            assert!(
                !path.as_os_str().is_empty(),
                "Watcher {} returned empty path in watch_paths",
                info.name
            );
        }
    }
}

/// Tests that is_available can be called without panicking.
///
/// is_available should gracefully return true or false based on whether
/// the tool is installed, not panic.
#[test]
fn test_all_watchers_is_available_does_not_panic() {
    let registry = default_registry();
    let watchers = registry.all_watchers();

    for watcher in watchers {
        // Should not panic
        let _ = watcher.is_available();
    }
}

/// Tests that the expected watchers are registered in the default registry.
#[test]
fn test_default_registry_contains_expected_watchers() {
    let registry = default_registry();

    let expected_watchers = [
        "aider",
        "amp",
        "claude-code",
        "cline",
        "codex",
        "continue",
        "gemini",
        "kilo-code",
        "opencode",
        "roo-code",
    ];

    for name in &expected_watchers {
        assert!(
            registry.get_watcher(name).is_some(),
            "Expected watcher '{}' not found in default registry",
            name
        );
    }
}

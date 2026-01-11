//! Common utilities shared across watcher implementations.
//!
//! This module provides helper functions for parsing timestamps, roles, and UUIDs
//! that are used by multiple watcher implementations. It also provides the
//! platform-specific path to VS Code's global storage directory.

use chrono::{DateTime, TimeZone, Utc};
use std::path::PathBuf;
use uuid::Uuid;

use crate::storage::models::MessageRole;

/// Returns the platform-specific path to VS Code's global storage directory.
///
/// This is where VS Code extensions store their data:
/// - macOS: `~/Library/Application Support/Code/User/globalStorage`
/// - Linux: `~/.config/Code/User/globalStorage`
/// - Windows: `%APPDATA%/Code/User/globalStorage`
pub fn vscode_global_storage() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Code/User/globalStorage")
    }
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Code/User/globalStorage")
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Code/User/globalStorage")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Code/User/globalStorage")
    }
}

/// Parses a role string into a MessageRole.
///
/// Handles common role names used across different AI tools:
/// - "user", "human" -> MessageRole::User
/// - "assistant" -> MessageRole::Assistant
/// - "system" -> MessageRole::System
///
/// Returns None for unrecognized roles.
pub fn parse_role(role: &str) -> Option<MessageRole> {
    match role {
        "user" | "human" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "system" => Some(MessageRole::System),
        _ => None,
    }
}

/// Parses a timestamp from milliseconds since Unix epoch.
///
/// Returns None if the timestamp is invalid or out of range.
pub fn parse_timestamp_millis(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

/// Parses a timestamp from an RFC3339 formatted string.
///
/// Returns None if the string cannot be parsed.
pub fn parse_timestamp_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Parses a string as a UUID, or generates a new one if parsing fails.
///
/// This is useful when importing sessions from tools that may use non-UUID
/// identifiers. The generated UUID is random and not deterministic.
#[allow(dead_code)]
pub fn parse_uuid_or_generate(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap_or_else(|_| Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vscode_global_storage_returns_valid_path() {
        let path = vscode_global_storage();
        // Should end with globalStorage
        assert!(path.to_string_lossy().contains("globalStorage"));
    }

    #[test]
    fn test_parse_role_user() {
        assert_eq!(parse_role("user"), Some(MessageRole::User));
    }

    #[test]
    fn test_parse_role_human() {
        assert_eq!(parse_role("human"), Some(MessageRole::User));
    }

    #[test]
    fn test_parse_role_assistant() {
        assert_eq!(parse_role("assistant"), Some(MessageRole::Assistant));
    }

    #[test]
    fn test_parse_role_system() {
        assert_eq!(parse_role("system"), Some(MessageRole::System));
    }

    #[test]
    fn test_parse_role_unknown() {
        assert_eq!(parse_role("unknown"), None);
        assert_eq!(parse_role(""), None);
        assert_eq!(parse_role("thinking"), None);
        assert_eq!(parse_role("tool"), None);
    }

    #[test]
    fn test_parse_timestamp_millis_valid() {
        let ts = parse_timestamp_millis(1704067200000);
        assert!(ts.is_some());
        let dt = ts.unwrap();
        assert_eq!(dt.timestamp_millis(), 1704067200000);
    }

    #[test]
    fn test_parse_timestamp_millis_zero() {
        let ts = parse_timestamp_millis(0);
        assert!(ts.is_some());
        assert_eq!(ts.unwrap().timestamp(), 0);
    }

    #[test]
    fn test_parse_timestamp_rfc3339_valid() {
        let ts = parse_timestamp_rfc3339("2025-01-15T10:00:00.000Z");
        assert!(ts.is_some());
        let dt = ts.unwrap();
        assert!(dt.to_rfc3339().contains("2025-01-15"));
    }

    #[test]
    fn test_parse_timestamp_rfc3339_with_offset() {
        let ts = parse_timestamp_rfc3339("2025-01-15T10:00:00-05:00");
        assert!(ts.is_some());
    }

    #[test]
    fn test_parse_timestamp_rfc3339_invalid() {
        assert!(parse_timestamp_rfc3339("not a timestamp").is_none());
        assert!(parse_timestamp_rfc3339("").is_none());
        assert!(parse_timestamp_rfc3339("2025-01-15").is_none());
    }

    #[test]
    fn test_parse_uuid_or_generate_valid_uuid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = parse_uuid_or_generate(uuid_str);
        assert_eq!(uuid.to_string(), uuid_str);
    }

    #[test]
    fn test_parse_uuid_or_generate_invalid_generates_new() {
        let uuid = parse_uuid_or_generate("not-a-uuid");
        assert!(!uuid.is_nil());
        // Should be a valid UUID v4
        assert_eq!(uuid.get_version_num(), 4);
    }

    #[test]
    fn test_parse_uuid_or_generate_empty_generates_new() {
        let uuid = parse_uuid_or_generate("");
        assert!(!uuid.is_nil());
    }
}

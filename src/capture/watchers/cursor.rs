//! Cursor IDE session parser.
//!
//! Parses conversation data from Cursor's SQLite databases. Cursor stores
//! AI chat conversations in workspace-specific databases at:
//!
//! - macOS: `~/Library/Application Support/Cursor/User/workspaceStorage/*/state.vscdb`
//! - Linux: `~/.config/Cursor/User/workspaceStorage/*/state.vscdb`
//! - Windows: `%APPDATA%/Cursor/User/workspaceStorage/*/state.vscdb`
//!
//! The database contains an `ItemTable` with key-value pairs where conversation
//! data is stored as JSON under keys matching `workbench.panel.aichat*`.

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Cursor IDE sessions.
///
/// Discovers and parses SQLite databases containing AI chat conversations
/// from Cursor's workspace storage.
pub struct CursorWatcher;

impl Watcher for CursorWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "cursor",
            description: "Cursor IDE AI conversations",
            default_paths: vec![cursor_storage_path()],
        }
    }

    fn is_available(&self) -> bool {
        cursor_storage_path().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_cursor_databases()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        parse_cursor_database(path)
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![cursor_storage_path()]
    }
}

/// Returns the path to Cursor's workspace storage directory.
///
/// This is platform-specific:
/// - macOS: `~/Library/Application Support/Cursor/User/workspaceStorage`
/// - Linux: `~/.config/Cursor/User/workspaceStorage`
/// - Windows: `%APPDATA%/Cursor/User/workspaceStorage`
fn cursor_storage_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Cursor/User/workspaceStorage")
    }
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cursor/User/workspaceStorage")
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cursor/User/workspaceStorage")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        // Fallback for other platforms
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Cursor/User/workspaceStorage")
    }
}

/// Discovers all Cursor database files in the workspace storage.
///
/// Scans workspace directories for `state.vscdb` files that may contain
/// AI conversation data.
pub fn find_cursor_databases() -> Result<Vec<PathBuf>> {
    let storage_path = cursor_storage_path();

    if !storage_path.exists() {
        return Ok(Vec::new());
    }

    let mut databases = Vec::new();

    for entry in std::fs::read_dir(&storage_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let db_path = path.join("state.vscdb");
            if db_path.exists() {
                databases.push(db_path);
            }
        }
    }

    Ok(databases)
}

/// Parses a Cursor database file and extracts AI conversations.
///
/// Opens the SQLite database and queries for conversation data stored
/// in the `ItemTable`. Each conversation is converted to a session with
/// its messages.
pub fn parse_cursor_database(path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
    let conn = Connection::open(path).context("Failed to open Cursor database")?;

    // Query for AI chat data - the key pattern may vary between Cursor versions
    let mut stmt = conn
        .prepare("SELECT key, value FROM ItemTable WHERE key LIKE 'workbench.panel.aichat%'")
        .context("Failed to prepare query")?;

    let mut sessions = Vec::new();

    let rows = stmt.query_map([], |row| {
        let key: String = row.get(0)?;
        let value: String = row.get(1)?;
        Ok((key, value))
    })?;

    for row in rows {
        let (key, value) = row?;
        tracing::debug!("Processing Cursor chat key: {}", key);

        // Try to parse as conversation data
        match parse_cursor_conversation(&value, path) {
            Ok(Some((session, messages))) => {
                if !messages.is_empty() {
                    sessions.push((session, messages));
                }
            }
            Ok(None) => {
                // Not a conversation entry or empty
            }
            Err(e) => {
                tracing::debug!("Failed to parse conversation from key {}: {}", key, e);
            }
        }
    }

    Ok(sessions)
}

/// Raw Cursor conversation structure.
///
/// This represents the JSON format stored in Cursor's database.
/// The structure may vary between versions, so we use optional fields.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorConversation {
    /// Unique identifier for the conversation
    #[serde(default)]
    id: Option<String>,

    /// List of messages in the conversation
    #[serde(default)]
    messages: Vec<CursorMessage>,

    /// Timestamp when the conversation was created
    #[serde(default)]
    created_at: Option<i64>,

    /// Timestamp when the conversation was last updated
    #[serde(default)]
    updated_at: Option<i64>,

    /// The workspace path this conversation belongs to
    #[serde(default)]
    workspace_path: Option<String>,
}

/// A message in a Cursor conversation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorMessage {
    /// Unique identifier for the message
    #[serde(default)]
    id: Option<String>,

    /// Role: "user" or "assistant"
    #[serde(default)]
    role: Option<String>,

    /// Message content
    #[serde(default)]
    content: Option<String>,

    /// Timestamp when the message was sent
    #[serde(default)]
    timestamp: Option<i64>,

    /// Alternative timestamp field name
    #[serde(default)]
    created_at: Option<i64>,
}

/// Parses a JSON value as a Cursor conversation.
///
/// Returns `Ok(Some(...))` if the value contains valid conversation data,
/// `Ok(None)` if it's not a conversation or is empty, or an error if parsing fails.
fn parse_cursor_conversation(
    json_value: &str,
    source_path: &Path,
) -> Result<Option<(Session, Vec<Message>)>> {
    // Try to parse as a conversation
    let conversation: CursorConversation = match serde_json::from_str(json_value) {
        Ok(c) => c,
        Err(_) => {
            // Try parsing as an array of conversations
            let conversations: Vec<CursorConversation> = match serde_json::from_str(json_value) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            // Take the first conversation if available
            match conversations.into_iter().next() {
                Some(c) => c,
                None => return Ok(None),
            }
        }
    };

    if conversation.messages.is_empty() {
        return Ok(None);
    }

    // Generate session ID from conversation ID or create a new one
    let session_id = conversation
        .id
        .as_ref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(Uuid::new_v4);

    // Determine timestamps
    let started_at = conversation
        .created_at
        .or_else(|| conversation.messages.first().and_then(|m| m.timestamp.or(m.created_at)))
        .and_then(|ts| Utc.timestamp_millis_opt(ts).single())
        .unwrap_or_else(Utc::now);

    let ended_at = conversation
        .updated_at
        .or_else(|| conversation.messages.last().and_then(|m| m.timestamp.or(m.created_at)))
        .and_then(|ts| Utc.timestamp_millis_opt(ts).single());

    // Create session
    let session = Session {
        id: session_id,
        tool: "cursor".to_string(),
        tool_version: None,
        started_at,
        ended_at,
        model: None,
        working_directory: conversation.workspace_path.unwrap_or_else(|| ".".to_string()),
        git_branch: None,
        source_path: Some(source_path.to_string_lossy().to_string()),
        message_count: conversation.messages.len() as i32,
    };

    // Convert messages
    let messages: Vec<Message> = conversation
        .messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| {
            let role = match msg.role.as_deref() {
                Some("user") => MessageRole::User,
                Some("assistant") => MessageRole::Assistant,
                Some("system") => MessageRole::System,
                _ => return None,
            };

            let content = msg.content.clone().unwrap_or_default();
            if content.is_empty() {
                return None;
            }

            let message_id = msg
                .id
                .as_ref()
                .and_then(|id| Uuid::parse_str(id).ok())
                .unwrap_or_else(Uuid::new_v4);

            let timestamp = msg
                .timestamp
                .or(msg.created_at)
                .and_then(|ts| Utc.timestamp_millis_opt(ts).single())
                .unwrap_or(started_at);

            Some(Message {
                id: message_id,
                session_id,
                parent_id: None,
                index: idx as i32,
                timestamp,
                role,
                content: MessageContent::Text(content),
                model: None,
                git_branch: None,
                cwd: None,
            })
        })
        .collect();

    if messages.is_empty() {
        return Ok(None);
    }

    Ok(Some((session, messages)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_storage_path_exists() {
        // Just verify the path is constructed properly
        let path = cursor_storage_path();
        assert!(path.to_string_lossy().contains("Cursor"));
        assert!(path.to_string_lossy().contains("workspaceStorage"));
    }

    #[test]
    fn test_watcher_info() {
        let watcher = CursorWatcher;
        let info = watcher.info();

        assert_eq!(info.name, "cursor");
        assert_eq!(info.description, "Cursor IDE AI conversations");
        assert!(!info.default_paths.is_empty());
    }

    #[test]
    fn test_watcher_watch_paths() {
        let watcher = CursorWatcher;
        let paths = watcher.watch_paths();

        assert!(!paths.is_empty());
        assert!(paths[0].to_string_lossy().contains("Cursor"));
    }

    #[test]
    fn test_find_databases_returns_empty_when_dir_missing() {
        // When Cursor is not installed, should return empty vec without error
        let result = find_cursor_databases();
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_cursor_conversation_valid() {
        let json = r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "messages": [
                {"id": "msg1", "role": "user", "content": "Hello", "timestamp": 1704067200000},
                {"id": "msg2", "role": "assistant", "content": "Hi there!", "timestamp": 1704067201000}
            ],
            "createdAt": 1704067200000,
            "workspacePath": "/home/user/project"
        }"#;

        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());

        let (session, messages) = result.unwrap().expect("Should have parsed conversation");
        assert_eq!(session.tool, "cursor");
        assert_eq!(session.working_directory, "/home/user/project");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_parse_cursor_conversation_empty_messages() {
        let json = r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "messages": []
        }"#;

        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_parse_cursor_conversation_invalid_json() {
        let json = "not valid json";
        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_parse_cursor_conversation_array_format() {
        let json = r#"[{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "messages": [
                {"role": "user", "content": "Test message"}
            ]
        }]"#;

        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());

        let (session, messages) = result.unwrap().expect("Should have parsed conversation");
        assert_eq!(session.tool, "cursor");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_parse_cursor_conversation_with_created_at_field() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": "Hello", "createdAt": 1704067200000}
            ]
        }"#;

        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());

        let (_, messages) = result.unwrap().expect("Should have parsed conversation");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_parse_cursor_conversation_filters_empty_content() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": ""},
                {"role": "user", "content": "Another message"}
            ]
        }"#;

        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());

        let (_, messages) = result.unwrap().expect("Should have parsed conversation");
        // Empty content message should be filtered out
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_parse_cursor_conversation_unknown_role() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "unknown_role", "content": "This should be skipped"}
            ]
        }"#;

        let source = PathBuf::from("/test/state.vscdb");
        let result = parse_cursor_conversation(json, &source);
        assert!(result.is_ok());

        let (_, messages) = result.unwrap().expect("Should have parsed conversation");
        assert_eq!(messages.len(), 1);
    }
}

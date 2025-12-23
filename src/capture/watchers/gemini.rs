//! Gemini CLI session parser.
//!
//! Parses session files from Google's Gemini CLI tool. Sessions are stored as
//! single JSON files at `~/.gemini/tmp/<project-hash>/chats/session-*.json`.
//!
//! Each file contains a JSON object with:
//! - `sessionId`: Unique session identifier
//! - `projectHash`: Hash of the project directory
//! - `startTime`: ISO 8601 timestamp
//! - `lastUpdated`: ISO 8601 timestamp
//! - `messages`: Array of message objects with id, timestamp, type, and content

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Gemini CLI sessions.
///
/// Discovers and parses JSON session files from the Gemini CLI tool.
/// Sessions are stored in `~/.gemini/tmp/<project-hash>/chats/session-*.json`.
pub struct GeminiWatcher;

impl Watcher for GeminiWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "gemini",
            description: "Google Gemini CLI",
            default_paths: vec![gemini_base_dir()],
        }
    }

    fn is_available(&self) -> bool {
        gemini_base_dir().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_gemini_session_files()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_gemini_session_file(path)?;
        if parsed.messages.is_empty() {
            return Ok(vec![]);
        }
        let (session, messages) = parsed.to_storage_models();
        Ok(vec![(session, messages)])
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![gemini_base_dir()]
    }
}

/// Returns the path to the Gemini base directory.
///
/// This is typically `~/.gemini/tmp/`.
fn gemini_base_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gemini")
        .join("tmp")
}

/// Raw session structure from Gemini JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGeminiSession {
    session_id: String,
    #[serde(default)]
    project_hash: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    last_updated: Option<String>,
    #[serde(default)]
    messages: Vec<RawGeminiMessage>,
}

/// Raw message structure from Gemini JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGeminiMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    content: Option<String>,
    // Optional fields we currently ignore but may use later
    #[serde(default)]
    #[allow(dead_code)]
    tool_calls: Option<serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    thoughts: Option<serde_json::Value>,
}

/// Parses a Gemini JSON session file.
///
/// Reads the JSON file and extracts session metadata and messages.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or parsed.
pub fn parse_gemini_session_file(path: &Path) -> Result<ParsedGeminiSession> {
    let content = fs::read_to_string(path).context("Failed to read Gemini session file")?;
    let raw: RawGeminiSession =
        serde_json::from_str(&content).context("Failed to parse Gemini session JSON")?;

    let start_time = raw
        .start_time
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let last_updated = raw
        .last_updated
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let messages: Vec<ParsedGeminiMessage> = raw
        .messages
        .iter()
        .filter_map(|m| {
            let role = match m.msg_type.as_str() {
                "user" => MessageRole::User,
                "gemini" => MessageRole::Assistant,
                "system" => MessageRole::System,
                _ => return None,
            };

            let content = m.content.as_ref()?.clone();
            if content.trim().is_empty() {
                return None;
            }

            let timestamp = m
                .timestamp
                .as_ref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .or(start_time)
                .unwrap_or_else(Utc::now);

            let id = m.id.clone();

            Some(ParsedGeminiMessage {
                id,
                timestamp,
                role,
                content,
            })
        })
        .collect();

    Ok(ParsedGeminiSession {
        session_id: raw.session_id,
        project_hash: raw.project_hash,
        start_time,
        last_updated,
        messages,
        source_path: path.to_string_lossy().to_string(),
    })
}

/// Intermediate representation of a parsed Gemini session.
#[derive(Debug)]
pub struct ParsedGeminiSession {
    pub session_id: String,
    pub project_hash: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
    pub messages: Vec<ParsedGeminiMessage>,
    pub source_path: String,
}

impl ParsedGeminiSession {
    /// Converts this parsed session to storage-ready models.
    pub fn to_storage_models(&self) -> (Session, Vec<Message>) {
        let session_uuid = Uuid::parse_str(&self.session_id).unwrap_or_else(|_| Uuid::new_v4());

        let started_at = self
            .start_time
            .or_else(|| self.messages.first().map(|m| m.timestamp))
            .unwrap_or_else(Utc::now);

        let ended_at = self
            .last_updated
            .or_else(|| self.messages.last().map(|m| m.timestamp));

        // Try to derive working directory from project hash in source path
        let working_directory = self
            .project_hash
            .as_ref()
            .map(|h| format!("<project:{h}>"))
            .unwrap_or_else(|| ".".to_string());

        let session = Session {
            id: session_uuid,
            tool: "gemini".to_string(),
            tool_version: None,
            started_at,
            ended_at,
            model: None,
            working_directory,
            git_branch: None,
            source_path: Some(self.source_path.clone()),
            message_count: self.messages.len() as i32,
        };

        let messages: Vec<Message> = self
            .messages
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let id =
                    m.id.as_ref()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .unwrap_or_else(Uuid::new_v4);

                Message {
                    id,
                    session_id: session_uuid,
                    parent_id: None,
                    index: idx as i32,
                    timestamp: m.timestamp,
                    role: m.role.clone(),
                    content: MessageContent::Text(m.content.clone()),
                    model: None,
                    git_branch: None,
                    cwd: None,
                }
            })
            .collect();

        (session, messages)
    }
}

/// Intermediate representation of a parsed Gemini message.
#[derive(Debug)]
pub struct ParsedGeminiMessage {
    pub id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub role: MessageRole,
    pub content: String,
}

/// Discovers all Gemini session files.
///
/// Scans `~/.gemini/tmp/*/chats/` for `session-*.json` files.
pub fn find_gemini_session_files() -> Result<Vec<PathBuf>> {
    let base_dir = gemini_base_dir();

    if !base_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    // Walk the directory tree: tmp/<project-hash>/chats/session-*.json
    for project_entry in std::fs::read_dir(&base_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let chats_dir = project_path.join("chats");
        if !chats_dir.exists() || !chats_dir.is_dir() {
            continue;
        }

        for file_entry in std::fs::read_dir(&chats_dir)? {
            let file_entry = file_entry?;
            let file_path = file_entry.path();

            if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("session-") && name.ends_with(".json") {
                    files.push(file_path);
                }
            }
        }
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Creates a temporary JSON file with given content.
    fn create_temp_session_file(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::with_suffix(".json").expect("Failed to create temp file");
        file.write_all(content.as_bytes())
            .expect("Failed to write content");
        file.flush().expect("Failed to flush");
        file
    }

    /// Generate a simple Gemini session JSON.
    fn make_session_json(session_id: &str, project_hash: &str, messages_json: &str) -> String {
        format!(
            r#"{{
                "sessionId": "{session_id}",
                "projectHash": "{project_hash}",
                "startTime": "2025-11-30T20:06:04.951Z",
                "lastUpdated": "2025-11-30T20:15:26.585Z",
                "messages": {messages_json}
            }}"#
        )
    }

    #[test]
    fn test_watcher_info() {
        let watcher = GeminiWatcher;
        let info = watcher.info();

        assert_eq!(info.name, "gemini");
        assert_eq!(info.description, "Google Gemini CLI");
        assert!(!info.default_paths.is_empty());
        assert!(info.default_paths[0].to_string_lossy().contains(".gemini"));
    }

    #[test]
    fn test_watcher_watch_paths() {
        let watcher = GeminiWatcher;
        let paths = watcher.watch_paths();

        assert!(!paths.is_empty());
        assert!(paths[0].to_string_lossy().contains(".gemini"));
    }

    #[test]
    fn test_parse_simple_session() {
        let json = make_session_json(
            "ed60a4d9-1234-5678-abcd-ef0123456789",
            "cc89a35",
            r#"[
                {"id": "msg1", "timestamp": "2025-11-30T20:06:05.000Z", "type": "user", "content": "Hello"},
                {"id": "msg2", "timestamp": "2025-11-30T20:06:10.000Z", "type": "gemini", "content": "Hi there!"}
            ]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.session_id, "ed60a4d9-1234-5678-abcd-ef0123456789");
        assert_eq!(parsed.project_hash, Some("cc89a35".to_string()));
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "Hello");
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[1].content, "Hi there!");
    }

    #[test]
    fn test_parse_user_message() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[{"type": "user", "content": "What is Rust?"}]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "What is Rust?");
    }

    #[test]
    fn test_parse_gemini_message_as_assistant() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[{"type": "gemini", "content": "Rust is a systems programming language."}]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
    }

    #[test]
    fn test_parse_system_message() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[{"type": "system", "content": "You are a helpful assistant."}]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_unknown_message_type_skipped() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[
                {"type": "user", "content": "Hello"},
                {"type": "unknown", "content": "Should be skipped"},
                {"type": "gemini", "content": "Hi!"}
            ]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_empty_content_skipped() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[
                {"type": "user", "content": "Hello"},
                {"type": "gemini", "content": ""},
                {"type": "gemini", "content": "   "},
                {"type": "user", "content": "Goodbye"}
            ]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 2);
    }

    #[test]
    fn test_null_content_skipped() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[
                {"type": "user", "content": "Hello"},
                {"type": "gemini"}
            ]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
    }

    #[test]
    fn test_to_storage_models() {
        let json = make_session_json(
            "ed60a4d9-1234-5678-abcd-ef0123456789",
            "cc89a35",
            r#"[
                {"id": "550e8400-e29b-41d4-a716-446655440001", "type": "user", "content": "Hello"},
                {"type": "gemini", "content": "Hi!"}
            ]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        assert_eq!(session.tool, "gemini");
        assert_eq!(
            session.id.to_string(),
            "ed60a4d9-1234-5678-abcd-ef0123456789"
        );
        assert!(session.working_directory.contains("cc89a35"));
        assert_eq!(session.message_count, 2);

        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].id.to_string(),
            "550e8400-e29b-41d4-a716-446655440001"
        );
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].index, 0);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].index, 1);
    }

    #[test]
    fn test_timestamps_parsed() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[{"type": "user", "content": "Hello", "timestamp": "2025-11-30T20:06:05.000Z"}]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert!(parsed.start_time.is_some());
        assert!(parsed.last_updated.is_some());
        assert!(parsed.messages[0]
            .timestamp
            .to_rfc3339()
            .contains("2025-11-30"));
    }

    #[test]
    fn test_empty_messages_array() {
        let json = make_session_json("test-session", "hash123", "[]");

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert!(parsed.messages.is_empty());
    }

    #[test]
    fn test_find_session_files_returns_empty_when_missing() {
        let result = find_gemini_session_files();
        assert!(result.is_ok());
        // May or may not find files depending on system
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = GeminiWatcher;
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[{"type": "user", "content": "Hello"}]"#,
        );

        let file = create_temp_session_file(&json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert_eq!(result.len(), 1);
        let (session, messages) = &result[0];
        assert_eq!(session.tool, "gemini");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source_empty_session() {
        let watcher = GeminiWatcher;
        let json = make_session_json("test-session", "hash123", "[]");

        let file = create_temp_session_file(&json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(result.is_empty());
    }

    #[test]
    fn test_invalid_uuid_generates_new() {
        let json = make_session_json(
            "not-a-valid-uuid",
            "hash123",
            r#"[{"type": "user", "content": "Hello"}]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");
        let (session, _) = parsed.to_storage_models();

        // Should still have a valid UUID (newly generated)
        assert!(!session.id.is_nil());
    }

    #[test]
    fn test_messages_with_tool_calls_and_thoughts() {
        let json = make_session_json(
            "test-session",
            "hash123",
            r#"[
                {
                    "type": "user",
                    "content": "Run a command",
                    "toolCalls": [{"name": "bash", "args": {"cmd": "ls"}}]
                },
                {
                    "type": "gemini",
                    "content": "Here are the files",
                    "thoughts": ["Analyzing directory structure"]
                }
            ]"#,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        // Should parse messages despite having extra fields
        assert_eq!(parsed.messages.len(), 2);
    }

    #[test]
    fn test_minimal_session() {
        let json = r#"{"sessionId": "minimal", "messages": []}"#;

        let file = create_temp_session_file(json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.session_id, "minimal");
        assert!(parsed.project_hash.is_none());
        assert!(parsed.messages.is_empty());
    }

    #[test]
    fn test_session_with_no_project_hash() {
        let json = r#"{
            "sessionId": "test",
            "startTime": "2025-11-30T20:06:04.951Z",
            "messages": [{"type": "user", "content": "Hello"}]
        }"#;

        let file = create_temp_session_file(json);
        let parsed = parse_gemini_session_file(file.path()).expect("Failed to parse");
        let (session, _) = parsed.to_storage_models();

        // Working directory should default to "."
        assert_eq!(session.working_directory, ".");
    }
}

//! Continue.dev session parser.
//!
//! Parses session data from Continue.dev, an open source AI coding assistant.
//! Sessions are stored as JSON files in `~/.continue/sessions/`.
//!
//! Each session file contains:
//! - Session ID and title
//! - Working directory
//! - Chat history with messages and context
//! - Optional model and usage information

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Continue.dev sessions.
///
/// Discovers and parses JSON session files from Continue.dev's storage.
/// Continue.dev is an open source VS Code extension for AI-assisted coding.
pub struct ContinueDevWatcher;

impl Watcher for ContinueDevWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "continue",
            description: "Continue.dev VS Code extension sessions",
            default_paths: vec![continue_sessions_path()],
        }
    }

    fn is_available(&self) -> bool {
        continue_sessions_path().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_continue_sessions()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_continue_session(path)?;
        match parsed {
            Some((session, messages)) if !messages.is_empty() => Ok(vec![(session, messages)]),
            _ => Ok(vec![]),
        }
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![continue_sessions_path()]
    }
}

/// Returns the path to Continue.dev's sessions directory.
///
/// This is typically `~/.continue/sessions/` on all platforms.
fn continue_sessions_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".continue")
        .join("sessions")
}

/// Finds all Continue.dev session files.
///
/// Scans the sessions directory for JSON files.
fn find_continue_sessions() -> Result<Vec<PathBuf>> {
    let sessions_path = continue_sessions_path();

    if !sessions_path.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    for entry in fs::read_dir(&sessions_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "json" {
                    files.push(path);
                }
            }
        }
    }

    Ok(files)
}

/// Raw Continue.dev session structure.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueSession {
    /// Session ID
    session_id: String,

    /// Working directory
    #[serde(default)]
    workspace_directory: Option<String>,

    /// Chat history
    #[serde(default)]
    history: Vec<ContinueChatHistoryItem>,

    /// Model used
    #[serde(default)]
    chat_model_title: Option<String>,
}

/// A chat history item in a Continue.dev session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueChatHistoryItem {
    /// The message
    message: ContinueChatMessage,
}

/// A message in Continue.dev chat history.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueChatMessage {
    /// Role: "user", "assistant", "system", "thinking", "tool"
    role: String,

    /// Message content (can be string or array of parts)
    content: ContinueMessageContent,
}

/// Message content in Continue.dev format.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ContinueMessageContent {
    /// Simple text content
    Text(String),
    /// Complex content with multiple parts
    Parts(Vec<ContinueMessagePart>),
}

impl ContinueMessageContent {
    /// Extracts text content from the message.
    fn to_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContinueMessagePart::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// A part of a Continue.dev message.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ContinueMessagePart {
    /// Text content
    Text { text: String },
    /// Image URL (we only match the variant, not use the inner fields)
    #[serde(rename = "imageUrl")]
    #[allow(dead_code)]
    ImageUrl {},
}

/// Parses a Continue.dev session file.
fn parse_continue_session(path: &Path) -> Result<Option<(Session, Vec<Message>)>> {
    let content = fs::read_to_string(path).context("Failed to read Continue session file")?;

    let raw_session: ContinueSession =
        serde_json::from_str(&content).context("Failed to parse Continue session JSON")?;

    if raw_session.history.is_empty() {
        return Ok(None);
    }

    // Parse session ID as UUID or generate new one
    let session_id = Uuid::parse_str(&raw_session.session_id).unwrap_or_else(|_| Uuid::new_v4());

    // Use file modification time for timestamps
    let file_mtime = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from);

    let ended_at = file_mtime;
    let message_count = raw_session.history.len();
    let started_at = ended_at
        .map(|t| t - chrono::Duration::minutes(message_count as i64 * 2))
        .unwrap_or_else(Utc::now);

    let session = Session {
        id: session_id,
        tool: "continue".to_string(),
        tool_version: None,
        started_at,
        ended_at,
        model: raw_session.chat_model_title,
        working_directory: raw_session
            .workspace_directory
            .unwrap_or_else(|| ".".to_string()),
        git_branch: None,
        source_path: Some(path.to_string_lossy().to_string()),
        message_count: message_count as i32,
    };

    // Convert messages
    let mut messages = Vec::new();
    let time_per_message = chrono::Duration::seconds(30);
    let mut current_time = started_at;

    for (idx, item) in raw_session.history.iter().enumerate() {
        let role = match item.message.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "thinking" => continue, // Skip thinking messages
            "tool" => continue,     // Skip tool result messages
            _ => continue,
        };

        let content = item.message.content.to_text();
        if content.trim().is_empty() {
            continue;
        }

        messages.push(Message {
            id: Uuid::new_v4(),
            session_id,
            parent_id: None,
            index: idx as i32,
            timestamp: current_time,
            role,
            content: MessageContent::Text(content),
            model: None,
            git_branch: None,
            cwd: Some(session.working_directory.clone()),
        });

        current_time += time_per_message;
    }

    if messages.is_empty() {
        return Ok(None);
    }

    Ok(Some((session, messages)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Creates a temporary Continue session file with given JSON content.
    fn create_temp_session_file(json: &str) -> NamedTempFile {
        let mut file = NamedTempFile::with_suffix(".json").expect("Failed to create temp file");
        file.write_all(json.as_bytes())
            .expect("Failed to write content");
        file.flush().expect("Failed to flush");
        file
    }

    #[test]
    fn test_watcher_info() {
        let watcher = ContinueDevWatcher;
        let info = watcher.info();

        assert_eq!(info.name, "continue");
        assert_eq!(info.description, "Continue.dev VS Code extension sessions");
    }

    #[test]
    fn test_watcher_watch_paths() {
        let watcher = ContinueDevWatcher;
        let paths = watcher.watch_paths();

        assert!(!paths.is_empty());
        assert!(paths[0].to_string_lossy().contains(".continue"));
        assert!(paths[0].to_string_lossy().contains("sessions"));
    }

    #[test]
    fn test_parse_simple_session() {
        let json = r#"{
            "sessionId": "550e8400-e29b-41d4-a716-446655440000",
            "title": "Test Session",
            "workspaceDirectory": "/home/user/project",
            "history": [
                {
                    "message": {
                        "role": "user",
                        "content": "Hello, can you help me?"
                    },
                    "contextItems": []
                },
                {
                    "message": {
                        "role": "assistant",
                        "content": "Of course! What do you need help with?"
                    },
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (session, messages) = result.expect("Should have session");
        assert_eq!(session.tool, "continue");
        assert_eq!(session.working_directory, "/home/user/project");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_parse_session_with_model() {
        let json = r#"{
            "sessionId": "test-session",
            "chatModelTitle": "GPT-4",
            "history": [
                {
                    "message": {
                        "role": "user",
                        "content": "Test"
                    },
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (session, _) = result.expect("Should have session");
        assert_eq!(session.model, Some("GPT-4".to_string()));
    }

    #[test]
    fn test_parse_empty_history() {
        let json = r#"{
            "sessionId": "test-session",
            "history": []
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        assert!(result.is_none());
    }

    #[test]
    fn test_parse_content_with_parts() {
        let json = r#"{
            "sessionId": "test-session",
            "history": [
                {
                    "message": {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "Hello"},
                            {"type": "text", "text": "World"}
                        ]
                    },
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        assert_eq!(messages.len(), 1);
        // Content parts should be joined
        if let MessageContent::Text(text) = &messages[0].content {
            assert!(text.contains("Hello"));
            assert!(text.contains("World"));
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_parse_skips_thinking_messages() {
        let json = r#"{
            "sessionId": "test-session",
            "history": [
                {
                    "message": {
                        "role": "user",
                        "content": "Question"
                    },
                    "contextItems": []
                },
                {
                    "message": {
                        "role": "thinking",
                        "content": "Thinking about this..."
                    },
                    "contextItems": []
                },
                {
                    "message": {
                        "role": "assistant",
                        "content": "Answer"
                    },
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        // Should only have user and assistant, not thinking
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_find_sessions_returns_ok_when_dir_missing() {
        let result = find_continue_sessions();
        assert!(result.is_ok());
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = ContinueDevWatcher;
        let json = r#"{
            "sessionId": "test",
            "history": [
                {
                    "message": {"role": "user", "content": "Test"},
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(!result.is_empty());
        let (session, _) = &result[0];
        assert_eq!(session.tool, "continue");
    }

    #[test]
    fn test_parse_filters_empty_content() {
        let json = r#"{
            "sessionId": "test-session",
            "history": [
                {
                    "message": {
                        "role": "user",
                        "content": "Hello"
                    },
                    "contextItems": []
                },
                {
                    "message": {
                        "role": "assistant",
                        "content": ""
                    },
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        // Empty content should be filtered out
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_session_id_parsing() {
        // Valid UUID
        let json = r#"{
            "sessionId": "550e8400-e29b-41d4-a716-446655440000",
            "history": [
                {
                    "message": {"role": "user", "content": "Test"},
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (session, _) = result.expect("Should have session");
        assert_eq!(
            session.id.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn test_session_id_fallback_for_invalid_uuid() {
        // Invalid UUID should generate a new one
        let json = r#"{
            "sessionId": "not-a-valid-uuid",
            "history": [
                {
                    "message": {"role": "user", "content": "Test"},
                    "contextItems": []
                }
            ]
        }"#;

        let file = create_temp_session_file(json);
        let result = parse_continue_session(file.path()).expect("Should parse");

        let (session, _) = result.expect("Should have session");
        // Should have a valid UUID (not nil)
        assert!(!session.id.is_nil());
    }
}

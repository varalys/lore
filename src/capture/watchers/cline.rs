//! Cline (Claude Dev) session parser.
//!
//! Parses conversation history from Cline, an AI coding assistant VS Code
//! extension formerly known as Claude Dev.
//!
//! Cline stores task conversations in the VS Code global storage directory:
//! - macOS: `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/`
//! - Linux: `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/`
//! - Windows: `%APPDATA%/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/`
//!
//! Each task has its own directory containing:
//! - `api_conversation_history.json` - Raw API message exchanges
//! - `ui_messages.json` - User-facing message format
//! - `task_metadata.json` - Task metadata (optional)

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Cline (Claude Dev) sessions.
///
/// Discovers and parses task conversation files from Cline's VS Code
/// extension storage.
pub struct ClineWatcher;

impl Watcher for ClineWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "cline",
            description: "Cline (Claude Dev) VS Code extension sessions",
            default_paths: vec![cline_tasks_path()],
        }
    }

    fn is_available(&self) -> bool {
        cline_tasks_path().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_cline_tasks()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_cline_task(path)?;
        match parsed {
            Some((session, messages)) if !messages.is_empty() => Ok(vec![(session, messages)]),
            _ => Ok(vec![]),
        }
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![cline_tasks_path()]
    }
}

/// Returns the path to Cline's tasks storage directory.
///
/// This is platform-specific:
/// - macOS: `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/tasks`
/// - Linux: `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks`
/// - Windows: `%APPDATA%/Code/User/globalStorage/saoudrizwan.claude-dev/tasks`
fn cline_tasks_path() -> PathBuf {
    let base = get_vscode_global_storage();
    base.join("saoudrizwan.claude-dev").join("tasks")
}

/// Returns the VS Code global storage path.
fn get_vscode_global_storage() -> PathBuf {
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

/// Finds all Cline task directories.
///
/// Each task has its own subdirectory containing conversation files.
fn find_cline_tasks() -> Result<Vec<PathBuf>> {
    let tasks_path = cline_tasks_path();

    if !tasks_path.exists() {
        return Ok(Vec::new());
    }

    let mut tasks = Vec::new();

    for entry in fs::read_dir(&tasks_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Check for conversation history file
            let history_file = path.join("api_conversation_history.json");
            if history_file.exists() {
                tasks.push(history_file);
            }
        }
    }

    Ok(tasks)
}

/// Raw Cline API conversation message.
#[derive(Debug, Deserialize)]
struct ClineApiMessage {
    /// Role: "user" or "assistant"
    role: String,

    /// Message content (can be string or array of content blocks)
    content: ClineContent,

    /// Timestamp (milliseconds since epoch)
    #[serde(default)]
    ts: Option<i64>,
}

/// Content in Cline API format.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ClineContent {
    /// Simple text content
    Text(String),
    /// Array of content blocks
    Blocks(Vec<ClineContentBlock>),
}

impl ClineContent {
    /// Extracts text content from the message.
    fn to_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ClineContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// A content block in Cline messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClineContentBlock {
    /// Text content
    Text { text: String },
    /// Image content
    Image {
        #[allow(dead_code)]
        source: serde_json::Value,
    },
    /// Tool use
    ToolUse {
        #[allow(dead_code)]
        id: Option<String>,
        #[allow(dead_code)]
        name: Option<String>,
        #[allow(dead_code)]
        input: Option<serde_json::Value>,
    },
    /// Tool result
    ToolResult {
        #[allow(dead_code)]
        tool_use_id: Option<String>,
        #[allow(dead_code)]
        content: Option<serde_json::Value>,
    },
}

/// Task metadata from Cline.
#[derive(Debug, Deserialize, Default)]
struct ClineTaskMetadata {
    /// Timestamp (ISO 8601 or milliseconds)
    #[serde(default)]
    ts: Option<serde_json::Value>,

    /// Working directory
    #[serde(default)]
    dir: Option<String>,
}

/// Parses a Cline task from its conversation history file.
fn parse_cline_task(history_path: &Path) -> Result<Option<(Session, Vec<Message>)>> {
    let content =
        fs::read_to_string(history_path).context("Failed to read Cline conversation history")?;

    let raw_messages: Vec<ClineApiMessage> =
        serde_json::from_str(&content).context("Failed to parse Cline conversation JSON")?;

    if raw_messages.is_empty() {
        return Ok(None);
    }

    // Try to get task directory for ID and metadata
    let task_dir = history_path.parent();
    let task_id = task_dir
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    // Try to read metadata
    let metadata = task_dir
        .map(|d| d.join("task_metadata.json"))
        .filter(|p| p.exists())
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str::<ClineTaskMetadata>(&c).ok())
        .unwrap_or_default();

    // Generate session ID from task ID or create new one
    let session_id = task_id
        .as_ref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(Uuid::new_v4);

    // Determine timestamps
    let first_ts = raw_messages.first().and_then(|m| m.ts);
    let last_ts = raw_messages.last().and_then(|m| m.ts);

    let started_at = first_ts
        .and_then(|ts| Utc.timestamp_millis_opt(ts).single())
        .or_else(|| {
            metadata.ts.as_ref().and_then(|v| match v {
                serde_json::Value::Number(n) => n
                    .as_i64()
                    .and_then(|ts| Utc.timestamp_millis_opt(ts).single()),
                serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc)),
                _ => None,
            })
        })
        .unwrap_or_else(Utc::now);

    let ended_at = last_ts.and_then(|ts| Utc.timestamp_millis_opt(ts).single());

    // Get working directory
    let working_directory = metadata
        .dir
        .or_else(|| {
            task_dir
                .and_then(|d| d.parent())
                .and_then(|d| d.parent())
                .and_then(|d| d.parent())
                .map(|d| d.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| ".".to_string());

    let session = Session {
        id: session_id,
        tool: "cline".to_string(),
        tool_version: None,
        started_at,
        ended_at,
        model: None,
        working_directory,
        git_branch: None,
        source_path: Some(history_path.to_string_lossy().to_string()),
        message_count: raw_messages.len() as i32,
        machine_id: crate::storage::get_machine_id(),
    };

    // Convert messages
    let mut messages = Vec::new();
    let time_per_message = chrono::Duration::seconds(30);
    let mut current_time = started_at;

    for (idx, msg) in raw_messages.iter().enumerate() {
        let role = match msg.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            _ => continue,
        };

        let content_text = msg.content.to_text();
        if content_text.trim().is_empty() {
            continue;
        }

        let timestamp = msg
            .ts
            .and_then(|ts| Utc.timestamp_millis_opt(ts).single())
            .unwrap_or(current_time);

        messages.push(Message {
            id: Uuid::new_v4(),
            session_id,
            parent_id: None,
            index: idx as i32,
            timestamp,
            role,
            content: MessageContent::Text(content_text),
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
    use tempfile::{NamedTempFile, TempDir};

    /// Creates a temporary Cline conversation file with given JSON content.
    fn create_temp_conversation_file(json: &str) -> NamedTempFile {
        let mut file = NamedTempFile::with_suffix(".json").expect("Failed to create temp file");
        file.write_all(json.as_bytes())
            .expect("Failed to write content");
        file.flush().expect("Failed to flush");
        file
    }

    /// Creates a temporary task directory structure.
    fn create_temp_task_dir(task_id: &str, history_json: &str) -> TempDir {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let task_dir = temp_dir.path().join(task_id);
        fs::create_dir_all(&task_dir).expect("Failed to create task dir");

        let history_file = task_dir.join("api_conversation_history.json");
        fs::write(&history_file, history_json).expect("Failed to write history file");

        temp_dir
    }

    // Note: Common watcher trait tests (info, watch_paths, find_sources) are in
    // src/capture/watchers/test_common.rs to avoid duplication across all watchers.
    // Only tool-specific parsing tests remain here.

    #[test]
    fn test_parse_simple_conversation() {
        let json = r#"[
            {"role": "user", "content": "Hello, can you help me?", "ts": 1704067200000},
            {"role": "assistant", "content": "Of course! What do you need?", "ts": 1704067230000}
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        let (session, messages) = result.expect("Should have session");
        assert_eq!(session.tool, "cline");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_parse_with_content_blocks() {
        let json = r#"[
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Hello"},
                    {"type": "text", "text": "World"}
                ],
                "ts": 1704067200000
            }
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        assert_eq!(messages.len(), 1);
        if let MessageContent::Text(text) = &messages[0].content {
            assert!(text.contains("Hello"));
            assert!(text.contains("World"));
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_parse_empty_conversation() {
        let json = "[]";

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        assert!(result.is_none());
    }

    #[test]
    fn test_parse_with_tool_blocks() {
        let json = r#"[
            {
                "role": "user",
                "content": "Create a file",
                "ts": 1704067200000
            },
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I'll create that file."},
                    {"type": "tool_use", "id": "tool_1", "name": "write_file", "input": {"path": "test.txt"}}
                ],
                "ts": 1704067230000
            }
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_parse_filters_empty_content() {
        let json = r#"[
            {"role": "user", "content": "Hello", "ts": 1704067200000},
            {"role": "assistant", "content": "", "ts": 1704067230000}
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        // Empty content should be filtered
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = ClineWatcher;
        let json = r#"[{"role": "user", "content": "Test", "ts": 1704067200000}]"#;

        let file = create_temp_conversation_file(json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(!result.is_empty());
        let (session, _) = &result[0];
        assert_eq!(session.tool, "cline");
    }

    #[test]
    fn test_parse_with_task_directory() {
        let json = r#"[
            {"role": "user", "content": "Hello", "ts": 1704067200000}
        ]"#;

        let temp_dir = create_temp_task_dir("550e8400-e29b-41d4-a716-446655440000", json);
        let history_path = temp_dir
            .path()
            .join("550e8400-e29b-41d4-a716-446655440000")
            .join("api_conversation_history.json");

        let result = parse_cline_task(&history_path).expect("Should parse");

        let (session, _) = result.expect("Should have session");
        assert_eq!(
            session.id.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn test_timestamps_from_messages() {
        let json = r#"[
            {"role": "user", "content": "First", "ts": 1704067200000},
            {"role": "assistant", "content": "Second", "ts": 1704067260000}
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        let (session, messages) = result.expect("Should have session");

        // started_at should be from first message
        assert!(session.started_at.timestamp_millis() == 1704067200000);

        // ended_at should be from last message
        assert!(session.ended_at.is_some());
        assert!(session.ended_at.unwrap().timestamp_millis() == 1704067260000);

        // Message timestamps should match
        assert!(messages[0].timestamp.timestamp_millis() == 1704067200000);
        assert!(messages[1].timestamp.timestamp_millis() == 1704067260000);
    }

    #[test]
    fn test_handles_unknown_role() {
        let json = r#"[
            {"role": "user", "content": "Hello", "ts": 1704067200000},
            {"role": "unknown", "content": "Should be skipped", "ts": 1704067230000}
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_cline_task(file.path()).expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        assert_eq!(messages.len(), 1);
    }
}

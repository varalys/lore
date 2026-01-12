//! Generic VS Code extension session parser.
//!
//! Provides a configurable watcher for VS Code extensions that use the Cline-style
//! task storage format. This includes Cline, Roo Code, and Kilo Code which all
//! store conversations in the same JSON format.
//!
//! Each task has its own directory containing:
//! - `api_conversation_history.json` - Raw API message exchanges
//! - `ui_messages.json` - User-facing message format (not parsed)
//! - `task_metadata.json` - Task metadata (optional)

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, Session};

use super::common::{
    parse_role, parse_timestamp_millis, parse_timestamp_rfc3339, vscode_global_storage,
};
use super::{Watcher, WatcherInfo};

/// Configuration for a VS Code extension watcher.
///
/// This struct holds the metadata needed to identify and describe
/// a specific VS Code extension that uses the Cline-style task format.
#[derive(Debug, Clone)]
pub struct VsCodeExtensionConfig {
    /// Short identifier for the watcher (e.g., "cline", "roo-code").
    pub name: &'static str,

    /// Human-readable description of the extension.
    pub description: &'static str,

    /// VS Code extension ID (e.g., "saoudrizwan.claude-dev").
    pub extension_id: &'static str,
}

/// A watcher for VS Code extensions that use the Cline-style task format.
///
/// This watcher can be configured to parse sessions from any VS Code extension
/// that stores conversations in the same format as Cline.
pub struct VsCodeExtensionWatcher {
    config: VsCodeExtensionConfig,
}

impl VsCodeExtensionWatcher {
    /// Creates a new watcher with the given configuration.
    pub fn new(config: VsCodeExtensionConfig) -> Self {
        Self { config }
    }

    /// Returns the path to the extension's tasks directory.
    fn tasks_path(&self) -> PathBuf {
        vscode_global_storage()
            .join(self.config.extension_id)
            .join("tasks")
    }
}

impl Watcher for VsCodeExtensionWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: self.config.name,
            description: self.config.description,
            default_paths: vec![self.tasks_path()],
        }
    }

    fn is_available(&self) -> bool {
        self.tasks_path().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_vscode_tasks(&self.tasks_path())
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_vscode_task(path, self.config.name)?;
        match parsed {
            Some((session, messages)) if !messages.is_empty() => Ok(vec![(session, messages)]),
            _ => Ok(vec![]),
        }
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![self.tasks_path()]
    }
}

/// Finds all task directories in the extension's tasks directory.
///
/// Each task has its own subdirectory containing conversation files.
pub fn find_vscode_tasks(tasks_path: &Path) -> Result<Vec<PathBuf>> {
    if !tasks_path.exists() {
        return Ok(Vec::new());
    }

    let mut tasks = Vec::new();

    for entry in fs::read_dir(tasks_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let history_file = path.join("api_conversation_history.json");
            if history_file.exists() {
                tasks.push(history_file);
            }
        }
    }

    Ok(tasks)
}

/// Raw API conversation message from VS Code extension storage.
#[derive(Debug, Deserialize)]
pub struct VsCodeApiMessage {
    /// Role: "user" or "assistant"
    pub role: String,

    /// Message content (can be string or array of content blocks)
    pub content: VsCodeContent,

    /// Timestamp (milliseconds since epoch)
    #[serde(default)]
    pub ts: Option<i64>,
}

/// Content in VS Code extension API format.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum VsCodeContent {
    /// Simple text content
    Text(String),
    /// Array of content blocks
    Blocks(Vec<VsCodeContentBlock>),
}

impl VsCodeContent {
    /// Extracts text content from the message.
    pub fn to_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    VsCodeContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// A content block in VS Code extension messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VsCodeContentBlock {
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

/// Task metadata from VS Code extension storage.
#[derive(Debug, Deserialize, Default)]
pub struct VsCodeTaskMetadata {
    /// Timestamp (ISO 8601 or milliseconds)
    #[serde(default)]
    pub ts: Option<serde_json::Value>,

    /// Working directory
    #[serde(default)]
    pub dir: Option<String>,
}

/// Parses a task from its conversation history file.
///
/// # Arguments
///
/// * `history_path` - Path to the `api_conversation_history.json` file
/// * `tool_name` - Name of the tool to use in the session (e.g., "cline")
pub fn parse_vscode_task(
    history_path: &Path,
    tool_name: &str,
) -> Result<Option<(Session, Vec<Message>)>> {
    let content =
        fs::read_to_string(history_path).context("Failed to read conversation history")?;

    let raw_messages: Vec<VsCodeApiMessage> =
        serde_json::from_str(&content).context("Failed to parse conversation JSON")?;

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
        .and_then(|c| serde_json::from_str::<VsCodeTaskMetadata>(&c).ok())
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
        .and_then(parse_timestamp_millis)
        .or_else(|| {
            metadata.ts.as_ref().and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_i64().and_then(parse_timestamp_millis),
                serde_json::Value::String(s) => parse_timestamp_rfc3339(s),
                _ => None,
            })
        })
        .unwrap_or_else(chrono::Utc::now);

    let ended_at = last_ts.and_then(parse_timestamp_millis);

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

    // Convert messages
    let mut messages = Vec::new();
    let time_per_message = chrono::Duration::seconds(30);
    let mut current_time = started_at;

    for (idx, msg) in raw_messages.iter().enumerate() {
        let role = match parse_role(&msg.role) {
            Some(r) => r,
            None => continue,
        };

        let content_text = msg.content.to_text();
        if content_text.trim().is_empty() {
            continue;
        }

        let timestamp = msg
            .ts
            .and_then(parse_timestamp_millis)
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
            cwd: Some(working_directory.clone()),
        });

        current_time += time_per_message;
    }

    if messages.is_empty() {
        return Ok(None);
    }

    let session = Session {
        id: session_id,
        tool: tool_name.to_string(),
        tool_version: None,
        started_at,
        ended_at,
        model: None,
        working_directory,
        git_branch: None,
        source_path: Some(history_path.to_string_lossy().to_string()),
        message_count: messages.len() as i32,
        machine_id: crate::storage::get_machine_id(),
    };

    Ok(Some((session, messages)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::MessageRole;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    /// Creates a temporary conversation file with given JSON content.
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

    #[test]
    fn test_parse_simple_conversation() {
        let json = r#"[
            {"role": "user", "content": "Hello, can you help me?", "ts": 1704067200000},
            {"role": "assistant", "content": "Of course! What do you need?", "ts": 1704067230000}
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

        let (session, messages) = result.expect("Should have session");
        assert_eq!(session.tool, "test-tool");
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
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

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
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

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
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

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
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        assert_eq!(messages.len(), 1);
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

        let result = parse_vscode_task(&history_path, "test-tool").expect("Should parse");

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
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

        let (session, messages) = result.expect("Should have session");

        assert_eq!(session.started_at.timestamp_millis(), 1704067200000);
        assert!(session.ended_at.is_some());
        assert_eq!(session.ended_at.unwrap().timestamp_millis(), 1704067260000);
        assert_eq!(messages[0].timestamp.timestamp_millis(), 1704067200000);
        assert_eq!(messages[1].timestamp.timestamp_millis(), 1704067260000);
    }

    #[test]
    fn test_handles_unknown_role() {
        let json = r#"[
            {"role": "user", "content": "Hello", "ts": 1704067200000},
            {"role": "unknown", "content": "Should be skipped", "ts": 1704067230000}
        ]"#;

        let file = create_temp_conversation_file(json);
        let result = parse_vscode_task(file.path(), "test-tool").expect("Should parse");

        let (_, messages) = result.expect("Should have session");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_info() {
        let config = VsCodeExtensionConfig {
            name: "test-ext",
            description: "Test extension",
            extension_id: "test.extension-id",
        };
        let watcher = VsCodeExtensionWatcher::new(config);
        let info = watcher.info();

        assert_eq!(info.name, "test-ext");
        assert_eq!(info.description, "Test extension");
    }

    #[test]
    fn test_watcher_parse_source() {
        let config = VsCodeExtensionConfig {
            name: "test-ext",
            description: "Test extension",
            extension_id: "test.extension-id",
        };
        let watcher = VsCodeExtensionWatcher::new(config);
        let json = r#"[{"role": "user", "content": "Test", "ts": 1704067200000}]"#;

        let file = create_temp_conversation_file(json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(!result.is_empty());
        let (session, _) = &result[0];
        assert_eq!(session.tool, "test-ext");
    }

    #[test]
    fn test_find_vscode_tasks_in_directory() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create two task directories
        let task1_dir = temp_dir.path().join("task-1");
        fs::create_dir_all(&task1_dir).expect("Failed to create task dir");
        fs::write(task1_dir.join("api_conversation_history.json"), "[]")
            .expect("Failed to write file");

        let task2_dir = temp_dir.path().join("task-2");
        fs::create_dir_all(&task2_dir).expect("Failed to create task dir");
        fs::write(task2_dir.join("api_conversation_history.json"), "[]")
            .expect("Failed to write file");

        // Create a task directory without history file (should be skipped)
        let task3_dir = temp_dir.path().join("task-3");
        fs::create_dir_all(&task3_dir).expect("Failed to create task dir");

        let tasks = find_vscode_tasks(temp_dir.path()).expect("Should find tasks");
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_find_vscode_tasks_nonexistent_dir() {
        let tasks = find_vscode_tasks(Path::new("/nonexistent/path")).expect("Should return empty");
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_vscode_content_to_text_simple() {
        let content = VsCodeContent::Text("Hello".to_string());
        assert_eq!(content.to_text(), "Hello");
    }

    #[test]
    fn test_vscode_content_to_text_blocks() {
        let content = VsCodeContent::Blocks(vec![
            VsCodeContentBlock::Text {
                text: "Hello".to_string(),
            },
            VsCodeContentBlock::ToolUse {
                id: Some("1".to_string()),
                name: Some("test".to_string()),
                input: None,
            },
            VsCodeContentBlock::Text {
                text: "World".to_string(),
            },
        ]);
        let text = content.to_text();
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("test")); // Tool use should not be included
    }
}

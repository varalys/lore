//! OpenCode CLI session parser.
//!
//! Parses session files from the OpenCode CLI tool (opencode.ai). OpenCode uses a
//! multi-file structure with separate directories for sessions, messages, and parts.
//!
//! Storage layout:
//! - Sessions: `~/.local/share/opencode/storage/session/<project-hash>/<session-id>.json`
//! - Messages: `~/.local/share/opencode/storage/message/<session-id>/msg_<id>.json`
//! - Parts: `~/.local/share/opencode/storage/part/msg_<id>/prt_<id>.json`
//!
//! Each session file contains metadata including project directory and timestamps.
//! Message files contain role and timing information. Part files contain the actual
//! text content or tool call information.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for OpenCode CLI sessions.
///
/// Discovers and parses session files from the OpenCode CLI tool.
/// Sessions are stored across multiple files in `~/.local/share/opencode/storage/`.
pub struct OpenCodeWatcher;

impl Watcher for OpenCodeWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "opencode",
            description: "OpenCode CLI",
            default_paths: vec![opencode_storage_dir()],
        }
    }

    fn is_available(&self) -> bool {
        opencode_storage_dir().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_opencode_session_files()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_opencode_session(path)?;
        if parsed.messages.is_empty() {
            return Ok(vec![]);
        }
        let (session, messages) = parsed.to_storage_models();
        Ok(vec![(session, messages)])
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![opencode_storage_dir()]
    }
}

/// Returns the path to the OpenCode storage directory.
///
/// OpenCode uses `~/.local/share/opencode/storage/` on all platforms.
fn opencode_storage_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("opencode")
        .join("storage")
}

/// Raw session structure from OpenCode JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpenCodeSession {
    id: String,
    #[serde(default)]
    version: Option<String>,
    // Parsed for potential use in project identification
    #[serde(default, rename = "projectID")]
    #[allow(dead_code)]
    project_id: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    time: Option<RawOpenCodeTime>,
}

/// Raw time structure from OpenCode JSON files.
#[derive(Debug, Deserialize)]
struct RawOpenCodeTime {
    created: i64,
    #[serde(default)]
    updated: Option<i64>,
}

/// Raw message structure from OpenCode JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpenCodeMessage {
    id: String,
    #[serde(rename = "sessionID")]
    session_id: String,
    role: String,
    #[serde(default)]
    time: Option<RawOpenCodeMessageTime>,
    #[serde(default, rename = "modelID")]
    model_id: Option<String>,
    // Parsed for potential use in provider-specific handling
    #[serde(default, rename = "providerID")]
    #[allow(dead_code)]
    provider_id: Option<String>,
    // For assistant messages, model info may be nested under "model" field
    #[serde(default)]
    model: Option<RawOpenCodeModel>,
}

/// Raw model structure for user messages.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpenCodeModel {
    #[serde(default, rename = "modelID")]
    model_id: Option<String>,
}

/// Raw message time structure from OpenCode JSON files.
#[derive(Debug, Deserialize)]
struct RawOpenCodeMessageTime {
    created: i64,
    // Parsed for potential use in duration calculation
    #[serde(default)]
    #[allow(dead_code)]
    completed: Option<i64>,
}

/// Raw part structure from OpenCode JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpenCodePart {
    #[serde(default)]
    id: Option<String>,
    // Parsed for potential use in validation
    #[serde(default, rename = "sessionID")]
    #[allow(dead_code)]
    session_id: Option<String>,
    // Parsed for potential use in validation
    #[serde(default, rename = "messageID")]
    #[allow(dead_code)]
    message_id: Option<String>,
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    state: Option<RawOpenCodeToolState>,
}

/// Raw tool state structure from OpenCode JSON files.
#[derive(Debug, Deserialize)]
struct RawOpenCodeToolState {
    #[serde(default)]
    status: Option<String>,
}

/// Parses an OpenCode session from its session file.
///
/// This function reads the session metadata and then loads all associated
/// messages and their parts from the multi-file structure.
///
/// # Errors
///
/// Returns an error if the session file cannot be read or parsed.
pub fn parse_opencode_session(session_path: &Path) -> Result<ParsedOpenCodeSession> {
    let content =
        fs::read_to_string(session_path).context("Failed to read OpenCode session file")?;
    let raw_session: RawOpenCodeSession =
        serde_json::from_str(&content).context("Failed to parse OpenCode session JSON")?;

    // Get the storage base directory (parent of session/<project-hash>/<session>.json)
    let storage_dir = session_path
        .parent() // <project-hash> dir
        .and_then(|p| p.parent()) // session dir
        .and_then(|p| p.parent()) // storage dir
        .unwrap_or_else(|| Path::new("."));

    // Parse session timestamps
    let created_at = raw_session
        .time
        .as_ref()
        .and_then(|t| Utc.timestamp_millis_opt(t.created).single());

    let updated_at = raw_session
        .time
        .as_ref()
        .and_then(|t| t.updated)
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single());

    // Load messages for this session
    let messages = load_session_messages(storage_dir, &raw_session.id)?;

    // Extract model from first assistant message
    let model = messages
        .iter()
        .find(|m| m.role == MessageRole::Assistant)
        .and_then(|m| m.model.clone());

    Ok(ParsedOpenCodeSession {
        session_id: raw_session.id,
        version: raw_session.version,
        title: raw_session.title,
        working_directory: raw_session.directory.unwrap_or_else(|| ".".to_string()),
        created_at,
        updated_at,
        model,
        messages,
        source_path: session_path.to_string_lossy().to_string(),
    })
}

/// Loads all messages for a session from the message directory.
fn load_session_messages(
    storage_dir: &Path,
    session_id: &str,
) -> Result<Vec<ParsedOpenCodeMessage>> {
    let message_dir = storage_dir.join("message").join(session_id);

    if !message_dir.exists() {
        return Ok(Vec::new());
    }

    let mut messages: Vec<(i64, ParsedOpenCodeMessage)> = Vec::new();

    for entry in fs::read_dir(&message_dir)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("msg_") && name.ends_with(".json") {
                if let Ok(msg) = parse_message_file(&path, storage_dir) {
                    // Use timestamp for sorting
                    let sort_key = msg.timestamp.timestamp_millis();
                    messages.push((sort_key, msg));
                }
            }
        }
    }

    // Sort by timestamp
    messages.sort_by_key(|(ts, _)| *ts);

    Ok(messages.into_iter().map(|(_, msg)| msg).collect())
}

/// Parses a single message file and loads its parts.
fn parse_message_file(path: &Path, storage_dir: &Path) -> Result<ParsedOpenCodeMessage> {
    let content = fs::read_to_string(path).context("Failed to read message file")?;
    let raw: RawOpenCodeMessage =
        serde_json::from_str(&content).context("Failed to parse message JSON")?;

    let role = match raw.role.as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => MessageRole::User,
    };

    let timestamp = raw
        .time
        .as_ref()
        .and_then(|t| Utc.timestamp_millis_opt(t.created).single())
        .unwrap_or_else(Utc::now);

    // Get model from either top-level modelId or nested model.modelId
    let model = raw
        .model_id
        .or_else(|| raw.model.as_ref().and_then(|m| m.model_id.clone()));

    // Load parts for this message
    let content = load_message_parts(storage_dir, &raw.id)?;

    Ok(ParsedOpenCodeMessage {
        id: raw.id,
        session_id: raw.session_id,
        timestamp,
        role,
        content,
        model,
    })
}

/// Loads all parts for a message and combines text parts into content.
fn load_message_parts(storage_dir: &Path, message_id: &str) -> Result<String> {
    let part_dir = storage_dir.join("part").join(message_id);

    if !part_dir.exists() {
        return Ok(String::new());
    }

    let mut parts: Vec<(String, String)> = Vec::new();

    for entry in fs::read_dir(&part_dir)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("prt_") && name.ends_with(".json") {
                if let Ok(part) = parse_part_file(&path) {
                    // Collect part ID and content for sorting
                    if let Some(id) = part.0 {
                        parts.push((id, part.1));
                    } else {
                        // No ID, just append with empty key
                        parts.push((String::new(), part.1));
                    }
                }
            }
        }
    }

    // Sort by part ID to maintain order
    parts.sort_by(|a, b| a.0.cmp(&b.0));

    // Combine text content
    let content: Vec<String> = parts.into_iter().map(|(_, text)| text).collect();
    Ok(content.join("\n"))
}

/// Parses a single part file and extracts text content.
///
/// Returns (part_id, text_content). Tool parts are converted to a summary string.
fn parse_part_file(path: &Path) -> Result<(Option<String>, String)> {
    let content = fs::read_to_string(path).context("Failed to read part file")?;
    let raw: RawOpenCodePart =
        serde_json::from_str(&content).context("Failed to parse part JSON")?;

    let text = match raw.part_type.as_str() {
        "text" => raw.text.unwrap_or_default(),
        "tool" => {
            // Include tool name and status as a summary
            let tool_name = raw.tool.unwrap_or_else(|| "unknown".to_string());
            let status = raw
                .state
                .as_ref()
                .and_then(|s| s.status.clone())
                .unwrap_or_else(|| "unknown".to_string());
            format!("[tool: {tool_name} ({status})]")
        }
        _ => String::new(),
    };

    Ok((raw.id, text))
}

/// Intermediate representation of a parsed OpenCode session.
#[derive(Debug)]
pub struct ParsedOpenCodeSession {
    pub session_id: String,
    pub version: Option<String>,
    // Parsed for potential future use in session display
    #[allow(dead_code)]
    pub title: Option<String>,
    pub working_directory: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub messages: Vec<ParsedOpenCodeMessage>,
    pub source_path: String,
}

impl ParsedOpenCodeSession {
    /// Converts this parsed session to storage-ready models.
    pub fn to_storage_models(&self) -> (Session, Vec<Message>) {
        // Generate a deterministic UUID from the session ID string
        let session_uuid = generate_uuid_from_string(&self.session_id);

        let started_at = self
            .created_at
            .or_else(|| self.messages.first().map(|m| m.timestamp))
            .unwrap_or_else(Utc::now);

        let ended_at = self
            .updated_at
            .or_else(|| self.messages.last().map(|m| m.timestamp));

        let session = Session {
            id: session_uuid,
            tool: "opencode".to_string(),
            tool_version: self.version.clone(),
            started_at,
            ended_at,
            model: self.model.clone(),
            working_directory: self.working_directory.clone(),
            git_branch: None,
            source_path: Some(self.source_path.clone()),
            message_count: self.messages.len() as i32,
            machine_id: crate::storage::get_machine_id(),
        };

        // Build message ID map for consistent UUIDs
        let message_uuid_map: HashMap<String, Uuid> = self
            .messages
            .iter()
            .map(|m| (m.id.clone(), generate_uuid_from_string(&m.id)))
            .collect();

        let messages: Vec<Message> = self
            .messages
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let id = *message_uuid_map.get(&m.id).unwrap_or(&Uuid::new_v4());

                Message {
                    id,
                    session_id: session_uuid,
                    parent_id: None,
                    index: idx as i32,
                    timestamp: m.timestamp,
                    role: m.role.clone(),
                    content: MessageContent::Text(m.content.clone()),
                    model: m.model.clone(),
                    git_branch: None,
                    cwd: None,
                }
            })
            .collect();

        (session, messages)
    }
}

/// Generates a deterministic UUID from a string by hashing it.
///
/// OpenCode uses identifiers like "ses_4b2a247aaffeEmXAKKN3BeRz2j" which are not
/// valid UUIDs. This function generates a consistent UUID using a simple hash
/// of the input string to ensure the same session ID always produces the same UUID.
fn generate_uuid_from_string(s: &str) -> Uuid {
    // First try to parse as a valid UUID
    if let Ok(uuid) = Uuid::parse_str(s) {
        return uuid;
    }

    // Generate a deterministic UUID by hashing the string.
    // We use a simple approach: hash the bytes and construct a UUID from the result.
    // This creates a "version 4 variant 1" UUID from the hash bytes.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    let hash1 = hasher.finish();

    // Hash again with a different seed for more bytes
    let mut hasher2 = DefaultHasher::new();
    hash1.hash(&mut hasher2);
    let hash2 = hasher2.finish();

    // Combine the two hashes into 16 bytes for a UUID
    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash1.to_le_bytes());
    bytes[8..16].copy_from_slice(&hash2.to_le_bytes());

    // Set version (4) and variant (1) bits to make it a valid UUID
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // Version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // Variant 1

    Uuid::from_bytes(bytes)
}

/// Intermediate representation of a parsed OpenCode message.
#[derive(Debug)]
pub struct ParsedOpenCodeMessage {
    pub id: String,
    // Stored for potential validation but not used in conversion
    #[allow(dead_code)]
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub role: MessageRole,
    pub content: String,
    pub model: Option<String>,
}

/// Discovers all OpenCode session files.
///
/// Scans `~/.local/share/opencode/storage/session/*/ses_*.json` for session files.
pub fn find_opencode_session_files() -> Result<Vec<PathBuf>> {
    let storage_dir = opencode_storage_dir();
    let session_dir = storage_dir.join("session");

    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    // Walk storage/session/<project-hash>/ses_*.json
    for project_entry in fs::read_dir(&session_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        for file_entry in fs::read_dir(&project_path)? {
            let file_entry = file_entry?;
            let file_path = file_entry.path();

            if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("ses_") && name.ends_with(".json") {
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
    use tempfile::TempDir;

    /// Creates a test OpenCode storage structure with session, message, and part files.
    struct TestOpenCodeStorage {
        _temp_dir: TempDir,
        storage_dir: PathBuf,
    }

    impl TestOpenCodeStorage {
        fn new() -> Self {
            let temp_dir = TempDir::new().expect("Failed to create temp dir");
            let storage_dir = temp_dir.path().join("storage");
            fs::create_dir_all(&storage_dir).expect("Failed to create storage dir");
            Self {
                _temp_dir: temp_dir,
                storage_dir,
            }
        }

        fn create_session(
            &self,
            project_hash: &str,
            session_id: &str,
            directory: &str,
            created_ms: i64,
        ) -> PathBuf {
            let session_dir = self.storage_dir.join("session").join(project_hash);
            fs::create_dir_all(&session_dir).expect("Failed to create session dir");

            let session_path = session_dir.join(format!("{session_id}.json"));
            let session_json = format!(
                r#"{{
                    "id": "{session_id}",
                    "version": "1.0.193",
                    "projectID": "{project_hash}",
                    "directory": "{directory}",
                    "title": "Test Session",
                    "time": {{
                        "created": {created_ms},
                        "updated": {updated_ms}
                    }}
                }}"#,
                updated_ms = created_ms + 10000
            );
            fs::write(&session_path, session_json).expect("Failed to write session file");
            session_path
        }

        fn create_message(
            &self,
            session_id: &str,
            message_id: &str,
            role: &str,
            created_ms: i64,
            model_id: Option<&str>,
        ) {
            let message_dir = self.storage_dir.join("message").join(session_id);
            fs::create_dir_all(&message_dir).expect("Failed to create message dir");

            let model_field = model_id
                .map(|m| format!(r#""modelID": "{m}","#))
                .unwrap_or_default();

            let message_json = format!(
                r#"{{
                    "id": "{message_id}",
                    "sessionID": "{session_id}",
                    "role": "{role}",
                    {model_field}
                    "time": {{
                        "created": {created_ms}
                    }}
                }}"#
            );
            let message_path = message_dir.join(format!("{message_id}.json"));
            fs::write(message_path, message_json).expect("Failed to write message file");
        }

        fn create_text_part(&self, message_id: &str, part_id: &str, text: &str) {
            let part_dir = self.storage_dir.join("part").join(message_id);
            fs::create_dir_all(&part_dir).expect("Failed to create part dir");

            // Note: OpenCode uses messageID with capital ID
            let part_json = format!(
                r#"{{
                    "id": "{part_id}",
                    "type": "text",
                    "text": "{text}"
                }}"#
            );
            let part_path = part_dir.join(format!("{part_id}.json"));
            fs::write(part_path, part_json).expect("Failed to write part file");
        }

        fn create_tool_part(&self, message_id: &str, part_id: &str, tool: &str, status: &str) {
            let part_dir = self.storage_dir.join("part").join(message_id);
            fs::create_dir_all(&part_dir).expect("Failed to create part dir");

            // Note: OpenCode uses messageID with capital ID
            let part_json = format!(
                r#"{{
                    "id": "{part_id}",
                    "type": "tool",
                    "tool": "{tool}",
                    "state": {{
                        "status": "{status}"
                    }}
                }}"#
            );
            let part_path = part_dir.join(format!("{part_id}.json"));
            fs::write(part_path, part_json).expect("Failed to write part file");
        }
    }

    // Note: Common watcher trait tests (info, watch_paths, find_sources) are in
    // src/capture/watchers/test_common.rs to avoid duplication across all watchers.
    // Only tool-specific parsing tests remain here.

    #[test]
    fn test_parse_simple_session() {
        let storage = TestOpenCodeStorage::new();
        let session_path = storage.create_session(
            "64ba75f0bc0e109e",
            "ses_test123",
            "/Users/test/project",
            1766529546325,
        );

        // Create a user message
        storage.create_message("ses_test123", "msg_user1", "user", 1766529546342, None);
        storage.create_text_part("msg_user1", "prt_user1", "Hello, OpenCode!");

        // Create an assistant message
        storage.create_message(
            "ses_test123",
            "msg_asst1",
            "assistant",
            1766529550000,
            Some("big-pickle"),
        );
        storage.create_text_part("msg_asst1", "prt_asst1", "Hello! How can I help you?");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.session_id, "ses_test123");
        assert_eq!(parsed.version, Some("1.0.193".to_string()));
        assert_eq!(parsed.working_directory, "/Users/test/project");
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "Hello, OpenCode!");
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[1].model, Some("big-pickle".to_string()));
    }

    #[test]
    fn test_parse_user_message() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_user_test", "/test/path", 1766529546325);

        storage.create_message("ses_user_test", "msg_u1", "user", 1766529546342, None);
        storage.create_text_part("msg_u1", "prt_u1", "What is Rust?");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "What is Rust?");
    }

    #[test]
    fn test_parse_assistant_message_with_model() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_asst_test", "/test/path", 1766529546325);

        storage.create_message(
            "ses_asst_test",
            "msg_a1",
            "assistant",
            1766529546342,
            Some("claude-opus-4"),
        );
        storage.create_text_part(
            "msg_a1",
            "prt_a1",
            "Rust is a systems programming language.",
        );

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[0].model, Some("claude-opus-4".to_string()));
        assert_eq!(parsed.model, Some("claude-opus-4".to_string()));
    }

    #[test]
    fn test_parse_tool_parts() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_tool_test", "/test/path", 1766529546325);

        storage.create_message(
            "ses_tool_test",
            "msg_t1",
            "assistant",
            1766529546342,
            Some("model"),
        );
        storage.create_text_part("msg_t1", "prt_t1a", "Let me read that file.");
        storage.create_tool_part("msg_t1", "prt_t1b", "read", "completed");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        // Content should include both text and tool summary
        assert!(parsed.messages[0]
            .content
            .contains("Let me read that file."));
        assert!(parsed.messages[0]
            .content
            .contains("[tool: read (completed)]"));
    }

    #[test]
    fn test_messages_sorted_by_timestamp() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_sort_test", "/test/path", 1766529546325);

        // Create messages out of order
        storage.create_message(
            "ses_sort_test",
            "msg_second",
            "assistant",
            1766529550000,
            None,
        );
        storage.create_text_part("msg_second", "prt_s", "Second message");

        storage.create_message("ses_sort_test", "msg_first", "user", 1766529546342, None);
        storage.create_text_part("msg_first", "prt_f", "First message");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].content, "First message");
        assert_eq!(parsed.messages[1].content, "Second message");
    }

    #[test]
    fn test_session_with_no_messages() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_empty", "/test/path", 1766529546325);

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert!(parsed.messages.is_empty());
    }

    #[test]
    fn test_to_storage_models() {
        let storage = TestOpenCodeStorage::new();
        let session_path = storage.create_session(
            "project123",
            "ses_storage_test",
            "/Users/test/project",
            1766529546325,
        );

        storage.create_message("ses_storage_test", "msg_u1", "user", 1766529546342, None);
        storage.create_text_part("msg_u1", "prt_u1", "Hello");

        storage.create_message(
            "ses_storage_test",
            "msg_a1",
            "assistant",
            1766529550000,
            Some("test-model"),
        );
        storage.create_text_part("msg_a1", "prt_a1", "Hi there!");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        assert_eq!(session.tool, "opencode");
        assert_eq!(session.tool_version, Some("1.0.193".to_string()));
        assert_eq!(session.working_directory, "/Users/test/project");
        assert_eq!(session.model, Some("test-model".to_string()));
        assert_eq!(session.message_count, 2);
        assert!(session.source_path.is_some());

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].index, 0);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].index, 1);
    }

    #[test]
    fn test_generate_uuid_from_string() {
        // Valid UUID should pass through
        let valid_uuid = "550e8400-e29b-41d4-a716-446655440000";
        let result = generate_uuid_from_string(valid_uuid);
        assert_eq!(result.to_string(), valid_uuid);

        // OpenCode-style ID should generate consistent UUID
        let opencode_id = "ses_4b2a247aaffeEmXAKKN3BeRz2j";
        let result1 = generate_uuid_from_string(opencode_id);
        let result2 = generate_uuid_from_string(opencode_id);
        assert_eq!(result1, result2);
        assert!(!result1.is_nil());
    }

    #[test]
    fn test_session_timestamps() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_time_test", "/test/path", 1766529546325);

        storage.create_message("ses_time_test", "msg_t1", "user", 1766529546342, None);
        storage.create_text_part("msg_t1", "prt_t1", "Hello");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");
        let (session, _) = parsed.to_storage_models();

        assert!(session.started_at.timestamp_millis() > 0);
        assert!(session.ended_at.is_some());
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = OpenCodeWatcher;
        let storage = TestOpenCodeStorage::new();
        let session_path = storage.create_session(
            "project123",
            "ses_watcher_test",
            "/test/path",
            1766529546325,
        );

        storage.create_message("ses_watcher_test", "msg_w1", "user", 1766529546342, None);
        storage.create_text_part("msg_w1", "prt_w1", "Hello");

        let result = watcher
            .parse_source(&session_path)
            .expect("Should parse successfully");

        assert_eq!(result.len(), 1);
        let (session, messages) = &result[0];
        assert_eq!(session.tool, "opencode");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source_empty_session() {
        let watcher = OpenCodeWatcher;
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_empty_test", "/test/path", 1766529546325);

        let result = watcher
            .parse_source(&session_path)
            .expect("Should parse successfully");

        assert!(result.is_empty());
    }

    #[test]
    fn test_multiple_text_parts_combined() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_multi_part", "/test/path", 1766529546325);

        storage.create_message("ses_multi_part", "msg_mp", "assistant", 1766529546342, None);
        storage.create_text_part("msg_mp", "prt_mp1", "First part.");
        storage.create_text_part("msg_mp", "prt_mp2", "Second part.");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        // Parts should be combined with newlines
        assert!(parsed.messages[0].content.contains("First part."));
        assert!(parsed.messages[0].content.contains("Second part."));
    }

    #[test]
    fn test_system_message() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_system", "/test/path", 1766529546325);

        storage.create_message("ses_system", "msg_sys", "system", 1766529546342, None);
        storage.create_text_part("msg_sys", "prt_sys", "You are a helpful assistant.");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_message_with_empty_parts_dir() {
        let storage = TestOpenCodeStorage::new();
        let session_path =
            storage.create_session("project123", "ses_no_parts", "/test/path", 1766529546325);

        storage.create_message("ses_no_parts", "msg_np", "user", 1766529546342, None);
        // Don't create any parts

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].content, "");
    }

    #[test]
    fn test_session_without_optional_fields() {
        let storage = TestOpenCodeStorage::new();

        // Create a minimal session file manually
        let session_dir = storage.storage_dir.join("session").join("minimal");
        fs::create_dir_all(&session_dir).expect("Failed to create session dir");

        let session_path = session_dir.join("ses_minimal.json");
        let session_json = r#"{"id": "ses_minimal"}"#;
        fs::write(&session_path, session_json).expect("Failed to write session file");

        let parsed = parse_opencode_session(&session_path).expect("Failed to parse");

        assert_eq!(parsed.session_id, "ses_minimal");
        assert_eq!(parsed.working_directory, ".");
        assert!(parsed.version.is_none());
    }

    #[test]
    fn test_find_session_files_in_storage() {
        let storage = TestOpenCodeStorage::new();

        // Create multiple sessions in different project directories
        storage.create_session("project_a", "ses_a1", "/path/a", 1766529546325);
        storage.create_session("project_a", "ses_a2", "/path/a", 1766529546325);
        storage.create_session("project_b", "ses_b1", "/path/b", 1766529546325);

        // Manually check the session directory exists
        let session_dir = storage.storage_dir.join("session");
        assert!(session_dir.exists());

        // Count session files
        let mut count = 0;
        for project_entry in fs::read_dir(&session_dir).unwrap() {
            let project_path = project_entry.unwrap().path();
            if project_path.is_dir() {
                for file_entry in fs::read_dir(&project_path).unwrap() {
                    let file_path = file_entry.unwrap().path();
                    if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("ses_") && name.ends_with(".json") {
                            count += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(count, 3);
    }
}

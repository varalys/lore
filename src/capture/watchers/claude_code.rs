//! Claude Code session parser.
//!
//! Parses the JSONL format used by Claude Code (as of version 2.0.72, December 2025).
//! Session files are stored in `~/.claude/projects/<project-hash>/<session-uuid>.jsonl`.
//!
//! Each line in a JSONL file represents a message or system event. This parser
//! extracts user and assistant messages while skipping file history snapshots
//! and sidechain (agent) messages.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{ContentBlock, Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Claude Code sessions.
///
/// Discovers and parses JSONL session files from the Claude Code CLI tool.
/// Sessions are stored in `~/.claude/projects/<project-hash>/<session-uuid>.jsonl`.
pub struct ClaudeCodeWatcher;

impl Watcher for ClaudeCodeWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "claude-code",
            description: "Claude Code CLI sessions",
            default_paths: vec![claude_projects_dir()],
        }
    }

    fn is_available(&self) -> bool {
        claude_projects_dir().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_session_files()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_session_file(path)?;
        if parsed.messages.is_empty() {
            return Ok(vec![]);
        }
        let (session, messages) = parsed.to_storage_models();
        Ok(vec![(session, messages)])
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![claude_projects_dir()]
    }
}

/// Returns the path to the Claude Code projects directory.
///
/// This is typically `~/.claude/projects/`.
fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

/// Raw message as stored in Claude Code JSONL files
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    #[serde(rename = "type")]
    msg_type: String,

    session_id: String,
    uuid: String,
    parent_uuid: Option<String>,
    timestamp: String,

    #[serde(default)]
    cwd: Option<String>,

    #[serde(default)]
    git_branch: Option<String>,

    #[serde(default)]
    version: Option<String>,

    #[serde(default)]
    message: Option<RawMessageContent>,

    // For agent/sidechain messages
    #[serde(default)]
    #[allow(dead_code)]
    agent_id: Option<String>,

    #[serde(default)]
    is_sidechain: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessageContent {
    role: String,

    #[serde(default)]
    model: Option<String>,

    content: RawContent,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawContent {
    Text(String),
    Blocks(Vec<RawContentBlock>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        #[allow(dead_code)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

/// Parses a Claude Code JSONL session file.
///
/// Reads each line of the file and extracts user and assistant messages.
/// Skips file history snapshots, sidechain messages, and malformed lines.
///
/// # Errors
///
/// Returns an error if the file cannot be opened. Individual malformed
/// lines are logged and skipped rather than causing a parse failure.
pub fn parse_session_file(path: &Path) -> Result<ParsedSession> {
    let file = File::open(path).context("Failed to open session file")?;
    let reader = BufReader::new(file);

    let mut messages: Vec<ParsedMessage> = Vec::new();
    let mut session_id: Option<String> = None;
    let mut tool_version: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut model: Option<String> = None;

    for (line_num, line) in reader.lines().enumerate() {
        let line = line.context(format!("Failed to read line {}", line_num + 1))?;

        if line.trim().is_empty() {
            continue;
        }

        // Try to parse as a message
        let raw: RawMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("Skipping unparseable line {}: {}", line_num + 1, e);
                continue;
            }
        };

        // Skip file-history-snapshot and other non-message types
        if raw.msg_type != "user" && raw.msg_type != "assistant" {
            continue;
        }

        // Skip sidechain/agent messages for now (they're in separate files anyway)
        if raw.is_sidechain.unwrap_or(false) {
            continue;
        }

        // Extract session metadata from first message
        if session_id.is_none() {
            session_id = Some(raw.session_id.clone());
        }
        if tool_version.is_none() {
            tool_version = raw.version.clone();
        }
        if cwd.is_none() {
            cwd = raw.cwd.clone();
        }
        if git_branch.is_none() {
            git_branch = raw.git_branch.clone();
        }

        // Parse the message content
        if let Some(ref msg_content) = raw.message {
            // Capture model from first assistant message
            if model.is_none() && msg_content.role == "assistant" {
                model = msg_content.model.clone();
            }

            let content = parse_content(&msg_content.content);
            let role = match msg_content.role.as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "system" => MessageRole::System,
                _ => MessageRole::User,
            };

            let timestamp = DateTime::parse_from_rfc3339(&raw.timestamp)
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            messages.push(ParsedMessage {
                uuid: raw.uuid,
                parent_uuid: raw.parent_uuid,
                timestamp,
                role,
                content,
                model: msg_content.model.clone(),
                git_branch: raw.git_branch,
                cwd: raw.cwd,
            });
        }
    }

    Ok(ParsedSession {
        session_id: session_id.unwrap_or_else(|| {
            // Try to get from filename
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        }),
        tool_version,
        cwd: cwd.unwrap_or_else(|| ".".to_string()),
        git_branch,
        model,
        messages,
        source_path: path.to_string_lossy().to_string(),
    })
}

fn parse_content(raw: &RawContent) -> MessageContent {
    match raw {
        RawContent::Text(s) => MessageContent::Text(s.clone()),
        RawContent::Blocks(blocks) => {
            let parsed: Vec<ContentBlock> = blocks
                .iter()
                .map(|b| match b {
                    RawContentBlock::Text { text } => ContentBlock::Text { text: text.clone() },
                    RawContentBlock::Thinking { thinking, .. } => ContentBlock::Thinking {
                        thinking: thinking.clone(),
                    },
                    RawContentBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                    RawContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: content.clone(),
                        is_error: *is_error,
                    },
                })
                .collect();
            MessageContent::Blocks(parsed)
        }
    }
}

/// Intermediate representation of a parsed session.
///
/// Contains all extracted data from a Claude Code session file before
/// conversion to storage models. Use [`to_storage_models`](Self::to_storage_models)
/// to convert to database-ready structures.
#[derive(Debug)]
pub struct ParsedSession {
    pub session_id: String,
    pub tool_version: Option<String>,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub messages: Vec<ParsedMessage>,
    pub source_path: String,
}

impl ParsedSession {
    /// Converts this parsed session to storage-ready models.
    ///
    /// Returns a tuple of `(Session, Vec<Message>)` suitable for database insertion.
    /// Generates UUIDs from the session ID string if valid, otherwise creates new ones.
    /// Also builds parent-child relationships between messages.
    pub fn to_storage_models(&self) -> (Session, Vec<Message>) {
        let session_uuid = Uuid::parse_str(&self.session_id).unwrap_or_else(|_| Uuid::new_v4());

        let started_at = self
            .messages
            .first()
            .map(|m| m.timestamp)
            .unwrap_or_else(Utc::now);

        let ended_at = self.messages.last().map(|m| m.timestamp);

        let session = Session {
            id: session_uuid,
            tool: "claude-code".to_string(),
            tool_version: self.tool_version.clone(),
            started_at,
            ended_at,
            model: self.model.clone(),
            working_directory: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            source_path: Some(self.source_path.clone()),
            message_count: self.messages.len() as i32,
            machine_id: crate::storage::get_machine_id(),
        };

        // Build UUID map for parent lookups
        let uuid_map: HashMap<String, Uuid> = self
            .messages
            .iter()
            .map(|m| {
                let uuid = Uuid::parse_str(&m.uuid).unwrap_or_else(|_| Uuid::new_v4());
                (m.uuid.clone(), uuid)
            })
            .collect();

        let messages: Vec<Message> = self
            .messages
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let id = *uuid_map.get(&m.uuid).unwrap();
                let parent_id = m
                    .parent_uuid
                    .as_ref()
                    .and_then(|p| uuid_map.get(p).copied());

                Message {
                    id,
                    session_id: session_uuid,
                    parent_id,
                    index: idx as i32,
                    timestamp: m.timestamp,
                    role: m.role.clone(),
                    content: m.content.clone(),
                    model: m.model.clone(),
                    git_branch: m.git_branch.clone(),
                    cwd: m.cwd.clone(),
                }
            })
            .collect();

        (session, messages)
    }
}

/// Intermediate representation of a parsed message.
///
/// Contains message data extracted from a Claude Code JSONL line.
/// Converted to the storage Message type via ParsedSession::to_storage_models.
#[derive(Debug)]
pub struct ParsedMessage {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub role: MessageRole,
    pub content: MessageContent,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
}

/// Discovers all Claude Code session files in `~/.claude/projects/`.
///
/// Scans project directories for UUID-named JSONL files, excluding
/// agent sidechain files. Returns an empty vector if the Claude
/// directory does not exist.
pub fn find_session_files() -> Result<Vec<PathBuf>> {
    let claude_dir = claude_projects_dir();

    if !claude_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    for entry in std::fs::read_dir(&claude_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Look for UUID-named JSONL files (not agent-* files for now)
            for file_entry in std::fs::read_dir(&path)? {
                let file_entry = file_entry?;
                let file_path = file_entry.path();

                if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                    // Skip agent files and non-jsonl files
                    if name.starts_with("agent-") {
                        continue;
                    }
                    if !name.ends_with(".jsonl") {
                        continue;
                    }
                    // Check if it looks like a UUID
                    if name.len() > 40 {
                        files.push(file_path);
                    }
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

    // =========================================================================
    // Helper functions to generate test JSONL lines
    // =========================================================================

    /// Generate a valid user message JSONL line
    fn make_user_message(
        session_id: &str,
        uuid: &str,
        parent_uuid: Option<&str>,
        content: &str,
    ) -> String {
        let parent = parent_uuid
            .map(|p| format!(r#""parentUuid":"{p}","#))
            .unwrap_or_default();
        format!(
            r#"{{"type":"user","sessionId":"{session_id}","uuid":"{uuid}",{parent}"timestamp":"2025-01-15T10:00:00.000Z","cwd":"/test/project","gitBranch":"main","version":"2.0.72","message":{{"role":"user","content":"{content}"}}}}"#
        )
    }

    /// Generate a valid assistant message JSONL line
    fn make_assistant_message(
        session_id: &str,
        uuid: &str,
        parent_uuid: Option<&str>,
        model: &str,
        content: &str,
    ) -> String {
        let parent = parent_uuid
            .map(|p| format!(r#""parentUuid": "{p}","#))
            .unwrap_or_default();
        format!(
            r#"{{"type":"assistant","sessionId":"{session_id}","uuid":"{uuid}",{parent}"timestamp":"2025-01-15T10:01:00.000Z","cwd":"/test/project","gitBranch":"main","message":{{"role":"assistant","model":"{model}","content":"{content}"}}}}"#
        )
    }

    /// Generate an assistant message with complex content blocks
    fn make_assistant_message_with_blocks(
        session_id: &str,
        uuid: &str,
        parent_uuid: Option<&str>,
        model: &str,
        blocks_json: &str,
    ) -> String {
        let parent = parent_uuid
            .map(|p| format!(r#""parentUuid": "{p}","#))
            .unwrap_or_default();
        format!(
            r#"{{"type":"assistant","sessionId":"{session_id}","uuid":"{uuid}",{parent}"timestamp":"2025-01-15T10:01:00.000Z","cwd":"/test/project","gitBranch":"main","message":{{"role":"assistant","model":"{model}","content":{blocks_json}}}}}"#
        )
    }

    /// Generate a system message JSONL line
    fn make_system_message(session_id: &str, uuid: &str, content: &str) -> String {
        format!(
            r#"{{"type":"user","sessionId":"{session_id}","uuid":"{uuid}","timestamp":"2025-01-15T09:59:00.000Z","cwd":"/test/project","message":{{"role":"system","content":"{content}"}}}}"#
        )
    }

    /// Generate a file-history-snapshot line (should be skipped)
    fn make_file_history_snapshot(session_id: &str, uuid: &str) -> String {
        format!(
            r#"{{"type":"file-history-snapshot","sessionId":"{session_id}","uuid":"{uuid}","timestamp":"2025-01-15T10:00:00.000Z","files":[]}}"#
        )
    }

    /// Generate a sidechain message (should be skipped)
    fn make_sidechain_message(session_id: &str, uuid: &str) -> String {
        format!(
            r#"{{"type":"user","sessionId":"{session_id}","uuid":"{uuid}","timestamp":"2025-01-15T10:00:00.000Z","isSidechain":true,"agentId":"agent-123","message":{{"role":"user","content":"sidechain message"}}}}"#
        )
    }

    /// Create a temporary JSONL file with given lines
    fn create_temp_session_file(lines: &[&str]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        for line in lines {
            writeln!(file, "{line}").expect("Failed to write line");
        }
        file.flush().expect("Failed to flush");
        file
    }

    // =========================================================================
    // Existing tests
    // =========================================================================

    #[test]
    fn test_parse_raw_content_text() {
        let raw = RawContent::Text("hello world".to_string());
        let content = parse_content(&raw);
        assert!(matches!(content, MessageContent::Text(s) if s == "hello world"));
    }

    #[test]
    fn test_parse_raw_content_blocks() {
        let json = r#"[{"type": "text", "text": "hello"}, {"type": "tool_use", "id": "123", "name": "Bash", "input": {"command": "ls"}}]"#;
        let blocks: Vec<RawContentBlock> = serde_json::from_str(json).unwrap();
        let raw = RawContent::Blocks(blocks);
        let content = parse_content(&raw);

        if let MessageContent::Blocks(blocks) = content {
            assert_eq!(blocks.len(), 2);
        } else {
            panic!("Expected blocks");
        }
    }

    // =========================================================================
    // Unit tests for JSONL parsing (valid input)
    // =========================================================================

    #[test]
    fn test_parse_valid_user_message() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let user_line = make_user_message(session_id, user_uuid, None, "Hello, Claude!");

        let file = create_temp_session_file(&[&user_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert!(
            matches!(&parsed.messages[0].content, MessageContent::Text(s) if s == "Hello, Claude!")
        );
        assert_eq!(parsed.messages[0].uuid, user_uuid);
    }

    #[test]
    fn test_parse_valid_assistant_message() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let assistant_uuid = "660e8400-e29b-41d4-a716-446655440002";
        let assistant_line = make_assistant_message(
            session_id,
            assistant_uuid,
            None,
            "claude-3-opus",
            "Hello! How can I help you?",
        );

        let file = create_temp_session_file(&[&assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
        assert!(
            matches!(&parsed.messages[0].content, MessageContent::Text(s) if s == "Hello! How can I help you?")
        );
        assert_eq!(parsed.messages[0].model, Some("claude-3-opus".to_string()));
    }

    #[test]
    fn test_session_metadata_extraction() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let assistant_uuid = "660e8400-e29b-41d4-a716-446655440002";

        let user_line = make_user_message(session_id, user_uuid, None, "Hello");
        let assistant_line = make_assistant_message(
            session_id,
            assistant_uuid,
            Some(user_uuid),
            "claude-opus-4",
            "Hi there!",
        );

        let file = create_temp_session_file(&[&user_line, &assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.session_id, session_id);
        assert_eq!(parsed.tool_version, Some("2.0.72".to_string()));
        assert_eq!(parsed.cwd, "/test/project");
        assert_eq!(parsed.git_branch, Some("main".to_string()));
        assert_eq!(parsed.model, Some("claude-opus-4".to_string()));
    }

    // =========================================================================
    // Unit tests for malformed JSONL handling
    // =========================================================================

    #[test]
    fn test_empty_lines_are_skipped() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let user_line = make_user_message(session_id, user_uuid, None, "Hello");

        let file = create_temp_session_file(&["", &user_line, "   ", ""]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].uuid, user_uuid);
    }

    #[test]
    fn test_invalid_json_is_gracefully_skipped() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let user_line = make_user_message(session_id, user_uuid, None, "Hello");

        let invalid_json = r#"{"this is not valid json"#;
        let another_invalid = r#"just plain text"#;
        let malformed_structure = r#"{"type": "user", "missing": "fields"}"#;

        let file = create_temp_session_file(&[
            invalid_json,
            &user_line,
            another_invalid,
            malformed_structure,
        ]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        // Should still have parsed the valid message
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].uuid, user_uuid);
    }

    #[test]
    fn test_unknown_message_types_are_skipped() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let snapshot_uuid = "770e8400-e29b-41d4-a716-446655440003";

        let user_line = make_user_message(session_id, user_uuid, None, "Hello");
        let snapshot_line = make_file_history_snapshot(session_id, snapshot_uuid);

        let file = create_temp_session_file(&[&snapshot_line, &user_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        // Only the user message should be parsed
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].uuid, user_uuid);
    }

    #[test]
    fn test_sidechain_messages_are_skipped() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let sidechain_uuid = "880e8400-e29b-41d4-a716-446655440004";

        let user_line = make_user_message(session_id, user_uuid, None, "Hello");
        let sidechain_line = make_sidechain_message(session_id, sidechain_uuid);

        let file = create_temp_session_file(&[&user_line, &sidechain_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].uuid, user_uuid);
    }

    // =========================================================================
    // Unit tests for message type parsing
    // =========================================================================

    #[test]
    fn test_parse_human_user_role() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440001";
        let user_line = make_user_message(session_id, uuid, None, "User message");

        let file = create_temp_session_file(&[&user_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages[0].role, MessageRole::User);
    }

    #[test]
    fn test_parse_assistant_role_with_model() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440002";
        let assistant_line =
            make_assistant_message(session_id, uuid, None, "claude-opus-4-5", "Response");

        let file = create_temp_session_file(&[&assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
        assert_eq!(
            parsed.messages[0].model,
            Some("claude-opus-4-5".to_string())
        );
    }

    #[test]
    fn test_parse_system_role() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440001";
        let system_line = make_system_message(session_id, uuid, "System instructions");

        let file = create_temp_session_file(&[&system_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_tool_use_blocks_parsed_correctly() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440002";

        let blocks_json = r#"[{"type":"text","text":"Let me run that command"},{"type":"tool_use","id":"tool_123","name":"Bash","input":{"command":"ls -la"}}]"#;
        let assistant_line = make_assistant_message_with_blocks(
            session_id,
            uuid,
            None,
            "claude-opus-4",
            blocks_json,
        );

        let file = create_temp_session_file(&[&assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        if let MessageContent::Blocks(blocks) = &parsed.messages[0].content {
            assert_eq!(blocks.len(), 2);

            // Check text block
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text == "Let me run that command")
            );

            // Check tool_use block
            if let ContentBlock::ToolUse { id, name, input } = &blocks[1] {
                assert_eq!(id, "tool_123");
                assert_eq!(name, "Bash");
                assert_eq!(input["command"], "ls -la");
            } else {
                panic!("Expected ToolUse block");
            }
        } else {
            panic!("Expected Blocks content");
        }
    }

    #[test]
    fn test_tool_result_blocks_parsed_correctly() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440001";

        // User messages can contain tool_result blocks
        let user_line = format!(
            r#"{{"type":"user","sessionId":"{session_id}","uuid":"{uuid}","timestamp":"2025-01-15T10:00:00.000Z","cwd":"/test","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"tool_123","content":"file1.txt\nfile2.txt","is_error":false}}]}}}}"#
        );

        let file = create_temp_session_file(&[&user_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        if let MessageContent::Blocks(blocks) = &parsed.messages[0].content {
            assert_eq!(blocks.len(), 1);

            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = &blocks[0]
            {
                assert_eq!(tool_use_id, "tool_123");
                assert_eq!(content, "file1.txt\nfile2.txt");
                assert!(!is_error);
            } else {
                panic!("Expected ToolResult block");
            }
        } else {
            panic!("Expected Blocks content");
        }
    }

    #[test]
    fn test_tool_result_with_error() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440001";

        let user_line = format!(
            r#"{{"type":"user","sessionId":"{session_id}","uuid":"{uuid}","timestamp":"2025-01-15T10:00:00.000Z","cwd":"/test","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"tool_456","content":"Command failed: permission denied","is_error":true}}]}}}}"#
        );

        let file = create_temp_session_file(&[&user_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        if let MessageContent::Blocks(blocks) = &parsed.messages[0].content {
            if let ContentBlock::ToolResult { is_error, .. } = &blocks[0] {
                assert!(*is_error);
            } else {
                panic!("Expected ToolResult block");
            }
        } else {
            panic!("Expected Blocks content");
        }
    }

    #[test]
    fn test_thinking_blocks_parsed_correctly() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = "660e8400-e29b-41d4-a716-446655440002";

        let blocks_json = r#"[{"type":"thinking","thinking":"Let me analyze this problem...","signature":"abc123"},{"type":"text","text":"Here is my answer"}]"#;
        let assistant_line = make_assistant_message_with_blocks(
            session_id,
            uuid,
            None,
            "claude-opus-4",
            blocks_json,
        );

        let file = create_temp_session_file(&[&assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        if let MessageContent::Blocks(blocks) = &parsed.messages[0].content {
            assert_eq!(blocks.len(), 2);

            // Check thinking block
            if let ContentBlock::Thinking { thinking } = &blocks[0] {
                assert_eq!(thinking, "Let me analyze this problem...");
            } else {
                panic!("Expected Thinking block");
            }

            // Check text block
            assert!(
                matches!(&blocks[1], ContentBlock::Text { text } if text == "Here is my answer")
            );
        } else {
            panic!("Expected Blocks content");
        }
    }

    // =========================================================================
    // Unit tests for session file discovery
    // =========================================================================
    //
    // Note: The test for find_sources handling missing directories is now in
    // src/capture/watchers/test_common.rs (test_all_watchers_find_sources_handles_missing_dirs)

    // =========================================================================
    // Test to_storage_models conversion
    // =========================================================================

    #[test]
    fn test_to_storage_models_creates_correct_session() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let assistant_uuid = "660e8400-e29b-41d4-a716-446655440002";

        let user_line = make_user_message(session_id, user_uuid, None, "Hello");
        let assistant_line = make_assistant_message(
            session_id,
            assistant_uuid,
            Some(user_uuid),
            "claude-opus-4",
            "Hi there!",
        );

        let file = create_temp_session_file(&[&user_line, &assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");
        let (session, _messages) = parsed.to_storage_models();

        // Verify session
        assert_eq!(session.id.to_string(), session_id);
        assert_eq!(session.tool, "claude-code");
        assert_eq!(session.tool_version, Some("2.0.72".to_string()));
        assert_eq!(session.model, Some("claude-opus-4".to_string()));
        assert_eq!(session.working_directory, "/test/project");
        assert_eq!(session.git_branch, Some("main".to_string()));
        assert_eq!(session.message_count, 2);
        assert!(session.source_path.is_some());

        // Verify started_at is from first message
        assert!(session.started_at.to_rfc3339().contains("2025-01-15T10:00"));

        // Verify ended_at is from last message
        assert!(session.ended_at.is_some());
        assert!(session
            .ended_at
            .unwrap()
            .to_rfc3339()
            .contains("2025-01-15T10:01"));
    }

    #[test]
    fn test_to_storage_models_creates_correct_messages() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let assistant_uuid = "660e8400-e29b-41d4-a716-446655440002";

        let user_line = make_user_message(session_id, user_uuid, None, "Hello");
        let assistant_line = make_assistant_message(
            session_id,
            assistant_uuid,
            Some(user_uuid),
            "claude-opus-4",
            "Hi there!",
        );

        let file = create_temp_session_file(&[&user_line, &assistant_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        assert_eq!(messages.len(), 2);

        // Verify first message (user)
        let user_msg = &messages[0];
        assert_eq!(user_msg.id.to_string(), user_uuid);
        assert_eq!(user_msg.session_id, session.id);
        assert!(user_msg.parent_id.is_none());
        assert_eq!(user_msg.index, 0);
        assert_eq!(user_msg.role, MessageRole::User);
        assert!(user_msg.model.is_none());

        // Verify second message (assistant)
        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg.id.to_string(), assistant_uuid);
        assert_eq!(assistant_msg.session_id, session.id);
        assert_eq!(assistant_msg.index, 1);
        assert_eq!(assistant_msg.role, MessageRole::Assistant);
        assert_eq!(assistant_msg.model, Some("claude-opus-4".to_string()));
    }

    #[test]
    fn test_to_storage_models_parent_id_linking() {
        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let uuid1 = "660e8400-e29b-41d4-a716-446655440001";
        let uuid2 = "660e8400-e29b-41d4-a716-446655440002";
        let uuid3 = "660e8400-e29b-41d4-a716-446655440003";

        let msg1 = make_user_message(session_id, uuid1, None, "First message");
        let msg2 = make_assistant_message(session_id, uuid2, Some(uuid1), "claude-opus-4", "Reply");
        let msg3 = make_user_message(session_id, uuid3, Some(uuid2), "Follow up");

        let file = create_temp_session_file(&[&msg1, &msg2, &msg3]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");
        let (_, messages) = parsed.to_storage_models();

        // First message has no parent
        assert!(messages[0].parent_id.is_none());

        // Second message's parent is first message
        assert_eq!(messages[1].parent_id, Some(messages[0].id));

        // Third message's parent is second message
        assert_eq!(messages[2].parent_id, Some(messages[1].id));
    }

    #[test]
    fn test_to_storage_models_with_invalid_uuid_generates_new() {
        // Test that invalid UUIDs are handled gracefully
        let session_id = "not-a-valid-uuid";
        let user_uuid = "also-not-valid";

        let user_line = format!(
            r#"{{"type":"user","sessionId":"{session_id}","uuid":"{user_uuid}","timestamp":"2025-01-15T10:00:00.000Z","cwd":"/test","message":{{"role":"user","content":"Hello"}}}}"#
        );

        let file = create_temp_session_file(&[&user_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        // Should still work with generated UUIDs
        assert!(!session.id.is_nil());
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].id.is_nil());
    }

    #[test]
    fn test_to_storage_models_empty_session() {
        // Create a session file with no valid messages
        let file = create_temp_session_file(&["", "  ", "invalid json"]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        assert!(messages.is_empty());
        assert_eq!(session.message_count, 0);
        // When no messages, started_at should be set to now (approximately)
        // and ended_at should be None
        assert!(session.ended_at.is_none());
    }

    #[test]
    fn test_session_id_from_filename_fallback() {
        // When session_id is not in any message, it should use the filename
        let invalid_line = r#"{"type":"unknown","sessionId":"","uuid":"test"}"#;

        let file = create_temp_session_file(&[invalid_line]);
        let parsed = parse_session_file(file.path()).expect("Failed to parse");

        // The session_id should be derived from the temp file name
        assert!(!parsed.session_id.is_empty());
        assert_ne!(parsed.session_id, "");
    }

    // =========================================================================
    // Tests for ClaudeCodeWatcher trait implementation
    // =========================================================================
    //
    // Note: Common watcher trait tests (info, watch_paths, find_sources) are in
    // src/capture/watchers/test_common.rs to avoid duplication across all watchers.
    // Only tool-specific parsing tests remain here.

    #[test]
    fn test_watcher_parse_source() {
        use super::Watcher;
        let watcher = ClaudeCodeWatcher;

        let session_id = "550e8400-e29b-41d4-a716-446655440000";
        let user_uuid = "660e8400-e29b-41d4-a716-446655440001";
        let user_line = make_user_message(session_id, user_uuid, None, "Hello");

        let file = create_temp_session_file(&[&user_line]);
        let path = file.path().to_path_buf();
        let result = watcher
            .parse_source(&path)
            .expect("Should parse successfully");

        assert_eq!(result.len(), 1);
        let (session, messages) = &result[0];
        assert_eq!(session.tool, "claude-code");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source_empty_session() {
        use super::Watcher;
        let watcher = ClaudeCodeWatcher;

        // Create a file with no valid messages
        let file = create_temp_session_file(&["", "invalid json"]);
        let path = file.path().to_path_buf();
        let result = watcher
            .parse_source(&path)
            .expect("Should parse successfully");

        // Empty sessions should return empty vec
        assert!(result.is_empty());
    }
}

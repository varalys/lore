//! Claude Code session parser
//!
//! Parses the JSONL format used by Claude Code (as of version 2.0.72, December 2025)

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use uuid::Uuid;

use crate::storage::models::{ContentBlock, Message, MessageContent, MessageRole, Session};

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

/// Parse a Claude Code session file
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

/// Parsed session data (before storage)
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
    /// Convert to storage models
    pub fn to_storage_models(&self) -> (Session, Vec<Message>) {
        let session_uuid = Uuid::parse_str(&self.session_id)
            .unwrap_or_else(|_| Uuid::new_v4());

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

/// Parsed message (intermediate representation)
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

/// Find all Claude Code session files
pub fn find_session_files() -> Result<Vec<std::path::PathBuf>> {
    let claude_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".claude")
        .join("projects");

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
}

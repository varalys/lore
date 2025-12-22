//! Core data models for Lore
//!
//! These represent the internal representation of reasoning history,
//! independent of any specific AI tool's format.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Session represents a complete human-AI collaboration.
/// This is the primary unit of reasoning history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique identifier for this session
    pub id: Uuid,

    /// Which tool created this session (e.g., "claude-code", "cursor")
    pub tool: String,

    /// Tool version (e.g., "2.0.72")
    pub tool_version: Option<String>,

    /// When the session started
    pub started_at: DateTime<Utc>,

    /// When the session ended (None if ongoing)
    pub ended_at: Option<DateTime<Utc>>,

    /// The AI model used (may change during session, this is the primary one)
    pub model: Option<String>,

    /// Working directory when session started
    pub working_directory: String,

    /// Git branch when session started (if in a git repo)
    pub git_branch: Option<String>,

    /// Original source file path (for re-import detection)
    pub source_path: Option<String>,

    /// Number of messages in this session
    pub message_count: i32,
}

/// A single message in a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique identifier for this message
    pub id: Uuid,

    /// Session this message belongs to
    pub session_id: Uuid,

    /// Parent message ID (for threading)
    pub parent_id: Option<Uuid>,

    /// Position in the conversation (0-indexed)
    pub index: i32,

    /// When this message was sent
    pub timestamp: DateTime<Utc>,

    /// Who sent this message
    pub role: MessageRole,

    /// The message content (may be complex for assistant messages)
    pub content: MessageContent,

    /// Model used (for assistant messages)
    pub model: Option<String>,

    /// Git branch at time of message
    pub git_branch: Option<String>,

    /// Working directory at time of message
    pub cwd: Option<String>,
}

/// The role of a message sender in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// A human user message.
    User,
    /// An AI assistant response.
    Assistant,
    /// A system prompt or instruction.
    System,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::User => write!(f, "user"),
            MessageRole::Assistant => write!(f, "assistant"),
            MessageRole::System => write!(f, "system"),
        }
    }
}

/// Message content - can be simple text or complex with tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text(String),
    /// Complex content with multiple blocks
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Get a text summary of the content
    #[allow(dead_code)]
    pub fn summary(&self, max_len: usize) -> String {
        let text = match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => {
                blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        ContentBlock::ToolUse { name, .. } => Some(format!("[tool: {name}]")),
                        ContentBlock::ToolResult { content, .. } => {
                            Some(format!("[result: {}...]", &content.chars().take(50).collect::<String>()))
                        }
                        ContentBlock::Thinking { .. } => None, // Skip thinking in summaries
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };

        if text.len() <= max_len {
            text
        } else {
            format!("{}...", &text.chars().take(max_len - 3).collect::<String>())
        }
    }

    /// Get the full text content (excluding tool calls and thinking)
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => {
                blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }
}

/// A block of content within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text
    Text { text: String },

    /// AI thinking/reasoning (may be redacted in display)
    Thinking { thinking: String },

    /// Tool/function call
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    /// Result from a tool call
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// Links a session to a git commit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLink {
    /// Unique identifier
    pub id: Uuid,

    /// Session being linked
    pub session_id: Uuid,

    /// Type of link
    pub link_type: LinkType,

    /// Git commit SHA (full)
    pub commit_sha: Option<String>,

    /// Branch name
    pub branch: Option<String>,

    /// Remote name (e.g., "origin")
    pub remote: Option<String>,

    /// When the link was created
    pub created_at: DateTime<Utc>,

    /// How the link was created
    pub created_by: LinkCreator,

    /// Confidence score for auto-links (0.0 - 1.0)
    pub confidence: Option<f64>,
}

/// The type of link between a session and git history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LinkType {
    /// Link to a specific commit.
    Commit,
    /// Link to a branch (session spans multiple commits).
    Branch,
    /// Link to a pull request.
    Pr,
    /// Manual link created by user without specific target.
    Manual,
}

/// Indicates how a session link was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LinkCreator {
    /// Automatically created by time and file overlap heuristics.
    Auto,
    /// Manually created by a user via CLI command.
    User,
}

/// A tracked git repository.
///
/// Repositories are discovered when sessions reference working directories
/// that are inside git repositories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    /// Unique identifier
    pub id: Uuid,

    /// Absolute path on disk
    pub path: String,

    /// Repository name (derived from path or remote)
    pub name: String,

    /// Remote URL if available
    pub remote_url: Option<String>,

    /// When first seen
    pub created_at: DateTime<Utc>,

    /// When last session was recorded
    pub last_session_at: Option<DateTime<Utc>>,
}

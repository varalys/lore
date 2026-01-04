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

    /// Machine identifier (hostname) where the session was captured.
    /// Used for cloud sync to identify which machine created the session.
    /// Optional for backwards compatibility with existing sessions.
    pub machine_id: Option<String>,
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
                        ContentBlock::ToolResult { content, .. } => Some(format!(
                            "[result: {}...]",
                            &content.chars().take(50).collect::<String>()
                        )),
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

    /// Get the full text content (excluding tool calls and thinking).
    ///
    /// For simple text messages, returns the text directly. For block content,
    /// extracts and concatenates all text blocks, ignoring tool calls and
    /// thinking blocks.
    pub fn text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
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

/// A search result from full-text search of message content.
///
/// Contains the matching message metadata along with a snippet of the
/// matching content for display in search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The session containing the matching message.
    pub session_id: Uuid,

    /// The matching message ID.
    pub message_id: Uuid,

    /// Role of the message sender (user, assistant, system).
    pub role: MessageRole,

    /// Snippet of matching content with search terms highlighted.
    pub snippet: String,

    /// Timestamp of the matching message.
    pub timestamp: DateTime<Utc>,

    /// Working directory of the session containing this message.
    pub working_directory: String,

    /// AI tool that captured this session (e.g., "claude-code", "aider").
    #[serde(default)]
    pub tool: String,

    /// Git branch name if available.
    #[serde(default)]
    pub git_branch: Option<String>,

    /// Total message count in the session.
    #[serde(default)]
    pub session_message_count: i32,

    /// When the session started.
    #[serde(default)]
    pub session_started_at: Option<DateTime<Utc>>,

    /// Index of the matching message within its session.
    #[serde(default)]
    pub message_index: i32,
}

/// Options for filtering search results.
///
/// Used by the search command to narrow down results by tool, date range,
/// project, branch, and other criteria. The context field is used by the
/// CLI layer, not the database layer.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct SearchOptions {
    /// The search query text.
    pub query: String,

    /// Maximum number of results to return.
    pub limit: usize,

    /// Filter by AI tool name (e.g., "claude-code", "aider").
    pub tool: Option<String>,

    /// Only include sessions after this date.
    pub since: Option<DateTime<Utc>>,

    /// Only include sessions before this date.
    pub until: Option<DateTime<Utc>>,

    /// Filter by project/directory name (partial match).
    pub project: Option<String>,

    /// Filter by git branch name (partial match).
    pub branch: Option<String>,

    /// Filter by message role (user, assistant, system).
    pub role: Option<String>,

    /// Filter by repository path prefix.
    pub repo: Option<String>,

    /// Number of context messages to include before and after matches.
    pub context: usize,
}

/// A search result with surrounding context messages.
///
/// Groups matches by session and includes neighboring messages for context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultWithContext {
    /// The session containing the matches.
    pub session_id: Uuid,

    /// AI tool that captured this session.
    pub tool: String,

    /// Project name (extracted from working directory).
    pub project: String,

    /// Full working directory path.
    pub working_directory: String,

    /// Git branch if available.
    pub git_branch: Option<String>,

    /// When the session started.
    pub session_started_at: DateTime<Utc>,

    /// Total messages in the session.
    pub session_message_count: i32,

    /// The matching messages with their context.
    pub matches: Vec<MatchWithContext>,
}

/// A single match with its surrounding context messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchWithContext {
    /// The matching message.
    pub message: ContextMessage,

    /// Messages before the match (for context).
    pub before: Vec<ContextMessage>,

    /// Messages after the match (for context).
    pub after: Vec<ContextMessage>,
}

/// A simplified message representation for context display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    /// Message ID.
    pub id: Uuid,

    /// Role of the sender.
    pub role: MessageRole,

    /// Text content (truncated for display).
    pub content: String,

    /// Message index in the session.
    pub index: i32,

    /// Whether this is the matching message.
    #[serde(default)]
    pub is_match: bool,
}

/// An annotation on a session.
///
/// Annotations are user-created bookmarks or notes attached to sessions.
/// They can be used to mark important moments, add context, or organize
/// sessions for later reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    /// Unique identifier for this annotation.
    pub id: Uuid,

    /// The session this annotation belongs to.
    pub session_id: Uuid,

    /// The annotation content (user-provided note or bookmark).
    pub content: String,

    /// When the annotation was created.
    pub created_at: DateTime<Utc>,
}

/// A tag applied to a session.
///
/// Tags provide a way to categorize and organize sessions. Each session
/// can have multiple tags, and the same tag label can be applied to
/// multiple sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    /// Unique identifier for this tag instance.
    pub id: Uuid,

    /// The session this tag is applied to.
    pub session_id: Uuid,

    /// The tag label (e.g., "bug-fix", "feature", "refactor").
    pub label: String,

    /// When the tag was applied.
    pub created_at: DateTime<Utc>,
}

/// A summary of a session.
///
/// Summaries provide a concise description of what happened in a session,
/// useful for quickly understanding the session context when continuing
/// work or reviewing history. Each session can have at most one summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    /// Unique identifier for this summary.
    pub id: Uuid,

    /// The session this summary describes.
    pub session_id: Uuid,

    /// The summary content text.
    pub content: String,

    /// When the summary was generated or last updated.
    pub generated_at: DateTime<Utc>,
}

/// Represents a machine that has captured sessions.
///
/// Used for cloud sync to map machine UUIDs to friendly names. Each machine
/// has a unique identifier (UUID) and a human-readable name that can be
/// customized by the user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Machine {
    /// Unique machine identifier (UUID).
    pub id: String,

    /// Human-readable machine name (e.g., hostname or custom name).
    pub name: String,

    /// When this machine was first registered (RFC3339 format).
    pub created_at: String,
}

/// A tracked git repository.
///
/// Repositories are discovered when sessions reference working directories
/// that are inside git repositories.
///
/// Note: This struct is defined for future use when repository tracking
/// is implemented. Currently, sessions link directly to commits without
/// explicit repository records.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
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

/// Extracts file paths mentioned in a list of messages.
///
/// Parses tool_use blocks to find file paths from tools like Read, Edit, Write,
/// Glob, and Bash commands. Returns unique file paths that were referenced.
///
/// # Arguments
///
/// * `messages` - The messages to extract file paths from
/// * `working_directory` - The session's working directory, used to convert
///   absolute paths to relative paths for comparison with git files
///
/// # Returns
///
/// A vector of unique file paths (relative to the working directory when possible).
pub fn extract_session_files(messages: &[Message], working_directory: &str) -> Vec<String> {
    use std::collections::HashSet;

    let mut files = HashSet::new();

    for message in messages {
        if let MessageContent::Blocks(blocks) = &message.content {
            for block in blocks {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    extract_files_from_tool_use(name, input, working_directory, &mut files);
                }
            }
        }
    }

    files.into_iter().collect()
}

/// Extracts file paths from a single tool_use block.
fn extract_files_from_tool_use(
    tool_name: &str,
    input: &serde_json::Value,
    working_directory: &str,
    files: &mut std::collections::HashSet<String>,
) {
    match tool_name {
        "Read" | "Write" | "Edit" => {
            // These tools have a file_path parameter
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                if let Some(rel_path) = make_relative(path, working_directory) {
                    files.insert(rel_path);
                }
            }
        }
        "Glob" => {
            // Glob has a path parameter for the directory to search
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                if let Some(rel_path) = make_relative(path, working_directory) {
                    files.insert(rel_path);
                }
            }
        }
        "Grep" => {
            // Grep has a path parameter
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                if let Some(rel_path) = make_relative(path, working_directory) {
                    files.insert(rel_path);
                }
            }
        }
        "Bash" => {
            // Try to extract file paths from bash commands
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                extract_files_from_bash_command(cmd, working_directory, files);
            }
        }
        "NotebookEdit" => {
            // NotebookEdit has a notebook_path parameter
            if let Some(path) = input.get("notebook_path").and_then(|v| v.as_str()) {
                if let Some(rel_path) = make_relative(path, working_directory) {
                    files.insert(rel_path);
                }
            }
        }
        _ => {}
    }
}

/// Extracts file paths from a bash command string.
///
/// This is a best-effort extraction that looks for common patterns.
fn extract_files_from_bash_command(
    cmd: &str,
    working_directory: &str,
    files: &mut std::collections::HashSet<String>,
) {
    // Common file-related commands
    let file_commands = [
        "cat", "less", "more", "head", "tail", "vim", "nano", "code", "cp", "mv", "rm", "touch",
        "mkdir", "chmod", "chown",
    ];

    // Split by common separators
    for part in cmd.split(&['|', ';', '&', '\n', ' '][..]) {
        let part = part.trim();

        // Check if this looks like a file path
        if part.starts_with('/') || part.starts_with("./") || part.starts_with("../") {
            // Skip if it's a command flag
            if !part.starts_with('-') {
                if let Some(rel_path) = make_relative(part, working_directory) {
                    // Only add if it looks like a reasonable file path
                    if !rel_path.is_empty() && !rel_path.contains('$') {
                        files.insert(rel_path);
                    }
                }
            }
        }

        // Check for file command patterns like "cat file.txt"
        for file_cmd in &file_commands {
            if part.starts_with(file_cmd) {
                let args = part.strip_prefix(file_cmd).unwrap_or("").trim();
                for arg in args.split_whitespace() {
                    // Skip flags
                    if arg.starts_with('-') {
                        continue;
                    }
                    // This might be a file path
                    if let Some(rel_path) = make_relative(arg, working_directory) {
                        if !rel_path.is_empty() && !rel_path.contains('$') {
                            files.insert(rel_path);
                        }
                    }
                }
            }
        }
    }
}

/// Converts an absolute path to a path relative to the working directory.
///
/// Returns None if the path cannot be made relative (e.g., not under working dir).
fn make_relative(path: &str, working_directory: &str) -> Option<String> {
    // Handle relative paths - they're already relative
    if !path.starts_with('/') {
        // Clean up "./" prefix if present
        let cleaned = path.strip_prefix("./").unwrap_or(path);
        if !cleaned.is_empty() {
            return Some(cleaned.to_string());
        }
        return None;
    }

    // For absolute paths, try to make them relative to working_directory
    let wd = working_directory.trim_end_matches('/');

    if let Some(rel) = path.strip_prefix(wd) {
        let rel = rel.trim_start_matches('/');
        if !rel.is_empty() {
            return Some(rel.to_string());
        }
    }

    // If we can't make it relative, still include it as-is
    // (git may use absolute paths in some cases)
    Some(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_files_read_tool() {
        let messages = vec![Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/home/user/project/src/main.rs"}),
            }]),
            model: None,
            git_branch: None,
            cwd: None,
        }];

        let files = extract_session_files(&messages, "/home/user/project");
        assert!(files.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_session_files_edit_tool() {
        let messages = vec![Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "Edit".to_string(),
                input: serde_json::json!({
                    "file_path": "/home/user/project/src/lib.rs",
                    "old_string": "old",
                    "new_string": "new"
                }),
            }]),
            model: None,
            git_branch: None,
            cwd: None,
        }];

        let files = extract_session_files(&messages, "/home/user/project");
        assert!(files.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_extract_session_files_multiple_tools() {
        let messages = vec![Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"file_path": "/project/a.rs"}),
                },
                ContentBlock::ToolUse {
                    id: "tool_2".to_string(),
                    name: "Write".to_string(),
                    input: serde_json::json!({"file_path": "/project/b.rs", "content": "..."}),
                },
                ContentBlock::ToolUse {
                    id: "tool_3".to_string(),
                    name: "Edit".to_string(),
                    input: serde_json::json!({
                        "file_path": "/project/c.rs",
                        "old_string": "x",
                        "new_string": "y"
                    }),
                },
            ]),
            model: None,
            git_branch: None,
            cwd: None,
        }];

        let files = extract_session_files(&messages, "/project");
        assert_eq!(files.len(), 3);
        assert!(files.contains(&"a.rs".to_string()));
        assert!(files.contains(&"b.rs".to_string()));
        assert!(files.contains(&"c.rs".to_string()));
    }

    #[test]
    fn test_extract_session_files_deduplicates() {
        let messages = vec![
            Message {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                parent_id: None,
                index: 0,
                timestamp: Utc::now(),
                role: MessageRole::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"file_path": "/project/src/main.rs"}),
                }]),
                model: None,
                git_branch: None,
                cwd: None,
            },
            Message {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                parent_id: None,
                index: 1,
                timestamp: Utc::now(),
                role: MessageRole::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "tool_2".to_string(),
                    name: "Edit".to_string(),
                    input: serde_json::json!({
                        "file_path": "/project/src/main.rs",
                        "old_string": "a",
                        "new_string": "b"
                    }),
                }]),
                model: None,
                git_branch: None,
                cwd: None,
            },
        ];

        let files = extract_session_files(&messages, "/project");
        assert_eq!(files.len(), 1);
        assert!(files.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_session_files_relative_paths() {
        let messages = vec![Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "./src/main.rs"}),
            }]),
            model: None,
            git_branch: None,
            cwd: None,
        }];

        let files = extract_session_files(&messages, "/project");
        assert!(files.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_session_files_empty_messages() {
        let messages: Vec<Message> = vec![];
        let files = extract_session_files(&messages, "/project");
        assert!(files.is_empty());
    }

    #[test]
    fn test_extract_session_files_text_only_messages() {
        let messages = vec![Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::User,
            content: MessageContent::Text("Please fix the bug".to_string()),
            model: None,
            git_branch: None,
            cwd: None,
        }];

        let files = extract_session_files(&messages, "/project");
        assert!(files.is_empty());
    }

    #[test]
    fn test_make_relative_absolute_path() {
        let result = make_relative("/home/user/project/src/main.rs", "/home/user/project");
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_make_relative_with_trailing_slash() {
        let result = make_relative("/home/user/project/src/main.rs", "/home/user/project/");
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_make_relative_already_relative() {
        let result = make_relative("src/main.rs", "/home/user/project");
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_make_relative_dotslash_prefix() {
        let result = make_relative("./src/main.rs", "/home/user/project");
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_make_relative_outside_working_dir() {
        let result = make_relative("/other/path/file.rs", "/home/user/project");
        // Should return the absolute path as-is
        assert_eq!(result, Some("/other/path/file.rs".to_string()));
    }
}

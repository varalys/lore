//! Amp CLI session parser.
//!
//! Parses session files from Sourcegraph's Amp CLI tool. Sessions are stored as
//! single JSON files at `~/.local/share/amp/threads/T-*.json`.
//!
//! Each file contains a JSON object with:
//! - `id`: Thread identifier with "T-" prefix followed by UUID
//! - `created`: Milliseconds since epoch
//! - `title`: Optional session title
//! - `messages`: Array of message objects with role, content, and metadata
//! - `env.initial.trees`: Array of project trees with working directory info

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{ContentBlock, Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Amp CLI sessions.
///
/// Discovers and parses JSON session files from the Amp CLI tool.
/// Sessions are stored in `~/.local/share/amp/threads/T-*.json`.
pub struct AmpWatcher;

impl Watcher for AmpWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "amp",
            description: "Amp CLI (Sourcegraph)",
            default_paths: vec![amp_threads_dir()],
        }
    }

    fn is_available(&self) -> bool {
        amp_threads_dir().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_amp_session_files()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_amp_session_file(path)?;
        if parsed.messages.is_empty() {
            return Ok(vec![]);
        }
        let (session, messages) = parsed.to_storage_models();
        Ok(vec![(session, messages)])
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![amp_threads_dir()]
    }
}

/// Returns the path to the Amp threads directory.
///
/// Amp uses `~/.local/share/amp/threads/` on all platforms (XDG-style).
fn amp_threads_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("amp")
        .join("threads")
}

/// Raw session structure from Amp JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAmpSession {
    id: String,
    created: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    messages: Vec<RawAmpMessage>,
    #[serde(default)]
    env: Option<RawAmpEnv>,
}

/// Raw environment structure from Amp JSON files.
#[derive(Debug, Deserialize)]
struct RawAmpEnv {
    #[serde(default)]
    initial: Option<RawAmpInitialEnv>,
}

/// Raw initial environment structure containing project trees.
#[derive(Debug, Deserialize)]
struct RawAmpInitialEnv {
    #[serde(default)]
    trees: Vec<RawAmpTree>,
}

/// Raw project tree structure from Amp JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAmpTree {
    // Parsed for potential future use in display, not currently used
    #[serde(default)]
    #[allow(dead_code)]
    display_name: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    repository: Option<RawAmpRepository>,
}

/// Raw repository structure from Amp JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAmpRepository {
    #[serde(rename = "ref")]
    #[serde(default)]
    git_ref: Option<String>,
}

/// Raw message structure from Amp JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAmpMessage {
    role: String,
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    content: Vec<RawAmpContentBlock>,
    #[serde(default)]
    meta: Option<RawAmpMessageMeta>,
    #[serde(default)]
    usage: Option<RawAmpUsage>,
}

/// Raw message metadata from Amp JSON files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAmpMessageMeta {
    #[serde(default)]
    sent_at: Option<i64>,
}

/// Raw usage information from Amp JSON files.
#[derive(Debug, Deserialize)]
struct RawAmpUsage {
    #[serde(default)]
    model: Option<String>,
}

/// Raw content block structure from Amp JSON files.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawAmpContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    #[serde(other)]
    Unknown,
}

/// Parses an Amp JSON session file.
///
/// Reads the JSON file and extracts session metadata and messages.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or parsed.
pub fn parse_amp_session_file(path: &Path) -> Result<ParsedAmpSession> {
    let content = fs::read_to_string(path).context("Failed to read Amp session file")?;
    let raw: RawAmpSession =
        serde_json::from_str(&content).context("Failed to parse Amp session JSON")?;

    // Parse session ID by stripping the "T-" prefix
    let session_id = raw.id.strip_prefix("T-").unwrap_or(&raw.id).to_string();

    // Convert created timestamp from milliseconds to DateTime
    let created_at = Utc.timestamp_millis_opt(raw.created).single();

    // Extract working directory from first tree's URI
    let working_directory = raw
        .env
        .as_ref()
        .and_then(|e| e.initial.as_ref())
        .and_then(|i| i.trees.first())
        .and_then(|t| t.uri.as_ref())
        .and_then(|uri| uri.strip_prefix("file://"))
        .map(String::from);

    // Extract git branch from first tree's repository ref
    let git_branch = raw
        .env
        .as_ref()
        .and_then(|e| e.initial.as_ref())
        .and_then(|i| i.trees.first())
        .and_then(|t| t.repository.as_ref())
        .and_then(|r| r.git_ref.as_ref())
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .map(String::from);

    // Parse messages
    let mut model: Option<String> = None;
    let messages: Vec<ParsedAmpMessage> = raw
        .messages
        .iter()
        .filter_map(|m| {
            let role = match m.role.as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "system" => MessageRole::System,
                _ => return None,
            };

            // Extract text content and thinking blocks
            let mut text_parts: Vec<String> = Vec::new();
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut has_thinking = false;

            for block in &m.content {
                match block {
                    RawAmpContentBlock::Text { text } => {
                        text_parts.push(text.clone());
                        content_blocks.push(ContentBlock::Text { text: text.clone() });
                    }
                    RawAmpContentBlock::Thinking { thinking } => {
                        has_thinking = true;
                        content_blocks.push(ContentBlock::Thinking {
                            thinking: thinking.clone(),
                        });
                    }
                    RawAmpContentBlock::Unknown => {}
                }
            }

            // Skip messages with no text content
            if text_parts.is_empty() && !has_thinking {
                return None;
            }

            // Capture model from first assistant message with usage info
            if model.is_none() && role == MessageRole::Assistant {
                model = m.usage.as_ref().and_then(|u| u.model.clone());
            }

            // Determine message content - use blocks if we have thinking, text otherwise
            let content = if has_thinking || content_blocks.len() > 1 {
                MessageContent::Blocks(content_blocks)
            } else {
                MessageContent::Text(text_parts.join("\n"))
            };

            // Get timestamp from meta.sentAt (milliseconds) or fall back to session created time
            let timestamp = m
                .meta
                .as_ref()
                .and_then(|meta| meta.sent_at)
                .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
                .or(created_at)
                .unwrap_or_else(Utc::now);

            Some(ParsedAmpMessage {
                message_id: m.message_id,
                timestamp,
                role,
                content,
                model: m.usage.as_ref().and_then(|u| u.model.clone()),
            })
        })
        .collect();

    Ok(ParsedAmpSession {
        session_id,
        title: raw.title,
        created_at,
        working_directory: working_directory.unwrap_or_else(|| ".".to_string()),
        git_branch,
        model,
        messages,
        source_path: path.to_string_lossy().to_string(),
    })
}

/// Intermediate representation of a parsed Amp session.
#[derive(Debug)]
pub struct ParsedAmpSession {
    pub session_id: String,
    // Parsed for potential future use in session display, not currently used
    #[allow(dead_code)]
    pub title: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub working_directory: String,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub messages: Vec<ParsedAmpMessage>,
    pub source_path: String,
}

impl ParsedAmpSession {
    /// Converts this parsed session to storage-ready models.
    pub fn to_storage_models(&self) -> (Session, Vec<Message>) {
        let session_uuid = Uuid::parse_str(&self.session_id).unwrap_or_else(|_| Uuid::new_v4());

        let started_at = self
            .created_at
            .or_else(|| self.messages.first().map(|m| m.timestamp))
            .unwrap_or_else(Utc::now);

        let ended_at = self.messages.last().map(|m| m.timestamp);

        let session = Session {
            id: session_uuid,
            tool: "amp".to_string(),
            tool_version: None,
            started_at,
            ended_at,
            model: self.model.clone(),
            working_directory: self.working_directory.clone(),
            git_branch: self.git_branch.clone(),
            source_path: Some(self.source_path.clone()),
            message_count: self.messages.len() as i32,
            machine_id: crate::storage::get_machine_id(),
        };

        let messages: Vec<Message> = self
            .messages
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let id = Uuid::new_v4();

                Message {
                    id,
                    session_id: session_uuid,
                    parent_id: None,
                    index: idx as i32,
                    timestamp: m.timestamp,
                    role: m.role.clone(),
                    content: m.content.clone(),
                    model: m.model.clone(),
                    git_branch: None,
                    cwd: None,
                }
            })
            .collect();

        (session, messages)
    }
}

/// Intermediate representation of a parsed Amp message.
#[derive(Debug)]
pub struct ParsedAmpMessage {
    // Parsed for potential future use in message ordering, not currently used
    #[allow(dead_code)]
    pub message_id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub role: MessageRole,
    pub content: MessageContent,
    pub model: Option<String>,
}

/// Discovers all Amp session files.
///
/// Scans `~/.local/share/amp/threads/` for `T-*.json` files.
pub fn find_amp_session_files() -> Result<Vec<PathBuf>> {
    let threads_dir = amp_threads_dir();

    if !threads_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    for entry in fs::read_dir(&threads_dir)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("T-") && name.ends_with(".json") {
                files.push(path);
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

    /// Generate a simple Amp session JSON.
    fn make_session_json(
        id: &str,
        created: i64,
        title: Option<&str>,
        messages_json: &str,
        env_json: Option<&str>,
    ) -> String {
        let title_str = title
            .map(|t| format!(r#""title": "{t}","#))
            .unwrap_or_default();
        let env_str = env_json
            .map(|e| format!(r#""env": {e},"#))
            .unwrap_or_default();
        format!(
            r#"{{
                "v": 235,
                "id": "{id}",
                "created": {created},
                {title_str}
                {env_str}
                "messages": {messages_json}
            }}"#
        )
    }

    fn make_env_json(uri: &str, git_ref: Option<&str>) -> String {
        let repo_str = git_ref
            .map(|r| format!(r#", "repository": {{"type": "git", "ref": "{r}"}}"#))
            .unwrap_or_default();
        format!(
            r#"{{
                "initial": {{
                    "trees": [{{
                        "displayName": "project",
                        "uri": "{uri}"{repo_str}
                    }}]
                }}
            }}"#
        )
    }

    #[test]
    fn test_watcher_info() {
        let watcher = AmpWatcher;
        let info = watcher.info();

        assert_eq!(info.name, "amp");
        assert_eq!(info.description, "Amp CLI (Sourcegraph)");
        assert!(!info.default_paths.is_empty());
        assert!(info.default_paths[0].to_string_lossy().contains("amp"));
    }

    #[test]
    fn test_watcher_watch_paths() {
        let watcher = AmpWatcher;
        let paths = watcher.watch_paths();

        assert!(!paths.is_empty());
        assert!(paths[0].to_string_lossy().contains("amp"));
        assert!(paths[0].to_string_lossy().contains("threads"));
    }

    #[test]
    fn test_parse_simple_session() {
        let json = make_session_json(
            "T-019b4d26-22b6-744d-8d30-d6bf43d6b520",
            1766525903546,
            Some("Test Session"),
            r#"[
                {
                    "role": "user",
                    "messageId": 0,
                    "content": [{"type": "text", "text": "Hello"}],
                    "meta": {"sentAt": 1766525916428}
                },
                {
                    "role": "assistant",
                    "messageId": 1,
                    "content": [{"type": "text", "text": "Hi there!"}],
                    "usage": {"model": "claude-opus-4-5-20251101", "inputTokens": 9, "outputTokens": 417}
                }
            ]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.session_id, "019b4d26-22b6-744d-8d30-d6bf43d6b520");
        assert_eq!(parsed.title, Some("Test Session".to_string()));
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
        assert_eq!(parsed.model, Some("claude-opus-4-5-20251101".to_string()));
    }

    #[test]
    fn test_parse_session_id_strips_prefix() {
        let json = make_session_json(
            "T-550e8400-e29b-41d4-a716-446655440000",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.session_id, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_parse_user_message() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "What is Rust?"}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content.text(), "What is Rust?");
    }

    #[test]
    fn test_parse_assistant_message_with_model() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{
                "role": "assistant",
                "content": [{"type": "text", "text": "Rust is a systems programming language."}],
                "usage": {"model": "claude-opus-4-5-20251101"}
            }]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
        assert_eq!(
            parsed.messages[0].model,
            Some("claude-opus-4-5-20251101".to_string())
        );
    }

    #[test]
    fn test_parse_thinking_blocks() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Let me analyze this..."},
                    {"type": "text", "text": "Here is my answer"}
                ]
            }]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        if let MessageContent::Blocks(blocks) = &parsed.messages[0].content {
            assert_eq!(blocks.len(), 2);
            assert!(
                matches!(&blocks[0], ContentBlock::Thinking { thinking } if thinking == "Let me analyze this...")
            );
            assert!(
                matches!(&blocks[1], ContentBlock::Text { text } if text == "Here is my answer")
            );
        } else {
            panic!("Expected Blocks content");
        }
    }

    #[test]
    fn test_parse_working_directory_from_env() {
        let env = make_env_json("file:///Users/franzer/projects/redactyl", None);
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            Some(&env),
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.working_directory, "/Users/franzer/projects/redactyl");
    }

    #[test]
    fn test_parse_git_branch_from_env() {
        let env = make_env_json(
            "file:///Users/franzer/projects/redactyl",
            Some("refs/heads/main"),
        );
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            Some(&env),
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.git_branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_message_timestamp_from_meta() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{
                "role": "user",
                "content": [{"type": "text", "text": "Hello"}],
                "meta": {"sentAt": 1766525916428}
            }]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        // The timestamp should be parsed from meta.sentAt
        assert!(parsed.messages[0].timestamp.timestamp_millis() > 0);
    }

    #[test]
    fn test_unknown_message_role_skipped() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[
                {"role": "user", "content": [{"type": "text", "text": "Hello"}]},
                {"role": "unknown", "content": [{"type": "text", "text": "Should be skipped"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi!"}]}
            ]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_empty_content_skipped() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[
                {"role": "user", "content": [{"type": "text", "text": "Hello"}]},
                {"role": "assistant", "content": []},
                {"role": "user", "content": [{"type": "text", "text": "Goodbye"}]}
            ]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 2);
    }

    #[test]
    fn test_to_storage_models() {
        let env = make_env_json(
            "file:///Users/franzer/projects/test",
            Some("refs/heads/feature"),
        );
        let json = make_session_json(
            "T-550e8400-e29b-41d4-a716-446655440000",
            1766525903546,
            Some("Test Title"),
            r#"[
                {"role": "user", "content": [{"type": "text", "text": "Hello"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi!"}], "usage": {"model": "claude-opus-4"}}
            ]"#,
            Some(&env),
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        assert_eq!(session.tool, "amp");
        assert_eq!(
            session.id.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(session.working_directory, "/Users/franzer/projects/test");
        assert_eq!(session.git_branch, Some("feature".to_string()));
        assert_eq!(session.model, Some("claude-opus-4".to_string()));
        assert_eq!(session.message_count, 2);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].index, 0);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].index, 1);
    }

    #[test]
    fn test_empty_messages_array() {
        let json = make_session_json("T-test-session", 1766525903546, None, "[]", None);

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert!(parsed.messages.is_empty());
    }

    #[test]
    fn test_find_session_files_returns_empty_when_missing() {
        let result = find_amp_session_files();
        assert!(result.is_ok());
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = AmpWatcher;
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert_eq!(result.len(), 1);
        let (session, messages) = &result[0];
        assert_eq!(session.tool, "amp");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source_empty_session() {
        let watcher = AmpWatcher;
        let json = make_session_json("T-test-session", 1766525903546, None, "[]", None);

        let file = create_temp_session_file(&json);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(result.is_empty());
    }

    #[test]
    fn test_invalid_uuid_generates_new() {
        let json = make_session_json(
            "T-not-a-valid-uuid",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");
        let (session, _) = parsed.to_storage_models();

        // Should still have a valid UUID (newly generated)
        assert!(!session.id.is_nil());
    }

    #[test]
    fn test_default_working_directory() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.working_directory, ".");
    }

    #[test]
    fn test_created_timestamp_parsing() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert!(parsed.created_at.is_some());
        assert!(parsed.created_at.unwrap().timestamp_millis() > 0);
    }

    #[test]
    fn test_system_message() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{"role": "system", "content": [{"type": "text", "text": "You are a helpful assistant."}]}]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_unknown_content_block_type_skipped() {
        let json = make_session_json(
            "T-test-session",
            1766525903546,
            None,
            r#"[{
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Hello"},
                    {"type": "tool_use", "id": "123", "name": "Bash"},
                    {"type": "text", "text": "World"}
                ]
            }]"#,
            None,
        );

        let file = create_temp_session_file(&json);
        let parsed = parse_amp_session_file(file.path()).expect("Failed to parse");

        // Should still parse the text blocks
        assert_eq!(parsed.messages.len(), 1);
        if let MessageContent::Blocks(blocks) = &parsed.messages[0].content {
            // Only text blocks should be present
            assert_eq!(blocks.len(), 2);
        } else {
            panic!("Expected Blocks content");
        }
    }
}

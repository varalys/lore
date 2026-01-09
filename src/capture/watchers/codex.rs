//! Codex CLI session parser.
//!
//! Parses session files from OpenAI's Codex CLI tool. Sessions are stored in
//! JSONL format at `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Each line in a JSONL file has a `type` field:
//! - `session_meta`: Contains session metadata (id, cwd, model, git info)
//! - `response_item`: Contains messages with role and content

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Codex CLI sessions.
///
/// Discovers and parses JSONL session files from the Codex CLI tool.
/// Sessions are stored in `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
pub struct CodexWatcher;

impl Watcher for CodexWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "codex",
            description: "OpenAI Codex CLI",
            default_paths: vec![codex_sessions_dir()],
        }
    }

    fn is_available(&self) -> bool {
        codex_sessions_dir().exists()
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_codex_session_files()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_codex_session_file(path)?;
        if parsed.messages.is_empty() {
            return Ok(vec![]);
        }
        let (session, messages) = parsed.to_storage_models();
        Ok(vec![(session, messages)])
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![codex_sessions_dir()]
    }
}

/// Returns the path to the Codex sessions directory.
///
/// This is typically `~/.codex/sessions/`.
fn codex_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("sessions")
}

/// Raw session metadata from Codex JSONL files.
#[derive(Debug, Deserialize)]
struct RawSessionMeta {
    id: String,
    #[allow(dead_code)]
    timestamp: String,
    cwd: String,
    #[serde(default)]
    cli_version: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    git: Option<RawGitInfo>,
}

/// Git information from session metadata.
#[derive(Debug, Deserialize)]
struct RawGitInfo {
    #[serde(default)]
    branch: Option<String>,
}

/// Raw entry from Codex JSONL files.
#[derive(Debug, Deserialize)]
struct RawEntry {
    timestamp: String,
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

/// Raw response item payload.
#[derive(Debug, Deserialize)]
struct RawResponseItem {
    #[serde(rename = "type")]
    item_type: Option<String>,
    role: Option<String>,
    #[serde(default)]
    content: Vec<RawContentItem>,
}

/// Raw content item within a response.
#[derive(Debug, Deserialize)]
struct RawContentItem {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Parses a Codex JSONL session file.
///
/// Reads each line of the file and extracts session metadata and messages.
/// Skips malformed lines rather than failing the entire parse.
///
/// # Errors
///
/// Returns an error if the file cannot be opened.
pub fn parse_codex_session_file(path: &Path) -> Result<ParsedCodexSession> {
    let file = File::open(path).context("Failed to open Codex session file")?;
    let reader = BufReader::new(file);

    let mut session_id: Option<String> = None;
    let mut cli_version: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut model_provider: Option<String> = None;
    let mut messages: Vec<ParsedCodexMessage> = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("Failed to read line {}: {}", line_num + 1, e);
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let entry: RawEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("Skipping unparseable line {}: {}", line_num + 1, e);
                continue;
            }
        };

        match entry.entry_type.as_str() {
            "session_meta" => {
                if let Some(payload) = entry.payload {
                    if let Ok(meta) = serde_json::from_value::<RawSessionMeta>(payload) {
                        if session_id.is_none() {
                            session_id = Some(meta.id);
                        }
                        if cli_version.is_none() {
                            cli_version = meta.cli_version;
                        }
                        if cwd.is_none() {
                            cwd = Some(meta.cwd);
                        }
                        if model_provider.is_none() {
                            model_provider = meta.model_provider;
                        }
                        if git_branch.is_none() {
                            git_branch = meta.git.and_then(|g| g.branch);
                        }
                    }
                }
            }
            "response_item" => {
                if let Some(payload) = entry.payload {
                    if let Ok(item) = serde_json::from_value::<RawResponseItem>(payload) {
                        // Only process message types
                        if item.item_type.as_deref() != Some("message") {
                            continue;
                        }

                        let role = match item.role.as_deref() {
                            Some("user") => MessageRole::User,
                            Some("assistant") => MessageRole::Assistant,
                            Some("system") => MessageRole::System,
                            _ => continue,
                        };

                        // Extract text content from content array
                        let text: String = item
                            .content
                            .iter()
                            .filter_map(|c| {
                                if c.content_type == "input_text" || c.content_type == "text" {
                                    c.text.clone()
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        if text.trim().is_empty() {
                            continue;
                        }

                        let timestamp = DateTime::parse_from_rfc3339(&entry.timestamp)
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now());

                        messages.push(ParsedCodexMessage {
                            timestamp,
                            role,
                            content: text,
                        });
                    }
                }
            }
            _ => {
                // Skip other entry types
            }
        }
    }

    Ok(ParsedCodexSession {
        session_id: session_id.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        }),
        cli_version,
        cwd: cwd.unwrap_or_else(|| ".".to_string()),
        git_branch,
        model_provider,
        messages,
        source_path: path.to_string_lossy().to_string(),
    })
}

/// Intermediate representation of a parsed Codex session.
#[derive(Debug)]
pub struct ParsedCodexSession {
    pub session_id: String,
    pub cli_version: Option<String>,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub model_provider: Option<String>,
    pub messages: Vec<ParsedCodexMessage>,
    pub source_path: String,
}

impl ParsedCodexSession {
    /// Converts this parsed session to storage-ready models.
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
            tool: "codex".to_string(),
            tool_version: self.cli_version.clone(),
            started_at,
            ended_at,
            model: self.model_provider.clone(),
            working_directory: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            source_path: Some(self.source_path.clone()),
            message_count: self.messages.len() as i32,
            machine_id: crate::storage::get_machine_id(),
        };

        let messages: Vec<Message> = self
            .messages
            .iter()
            .enumerate()
            .map(|(idx, m)| Message {
                id: Uuid::new_v4(),
                session_id: session_uuid,
                parent_id: None,
                index: idx as i32,
                timestamp: m.timestamp,
                role: m.role.clone(),
                content: MessageContent::Text(m.content.clone()),
                model: self.model_provider.clone(),
                git_branch: self.git_branch.clone(),
                cwd: Some(self.cwd.clone()),
            })
            .collect();

        (session, messages)
    }
}

/// Intermediate representation of a parsed Codex message.
#[derive(Debug)]
pub struct ParsedCodexMessage {
    pub timestamp: DateTime<Utc>,
    pub role: MessageRole,
    pub content: String,
}

/// Discovers all Codex session files.
///
/// Scans `~/.codex/sessions/YYYY/MM/DD/` for `rollout-*.jsonl` files.
pub fn find_codex_session_files() -> Result<Vec<PathBuf>> {
    let sessions_dir = codex_sessions_dir();

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    // Walk the directory tree: sessions/YYYY/MM/DD/rollout-*.jsonl
    for year_entry in std::fs::read_dir(&sessions_dir)? {
        let year_entry = year_entry?;
        let year_path = year_entry.path();
        if !year_path.is_dir() {
            continue;
        }

        for month_entry in std::fs::read_dir(&year_path)? {
            let month_entry = month_entry?;
            let month_path = month_entry.path();
            if !month_path.is_dir() {
                continue;
            }

            for day_entry in std::fs::read_dir(&month_path)? {
                let day_entry = day_entry?;
                let day_path = day_entry.path();
                if !day_path.is_dir() {
                    continue;
                }

                for file_entry in std::fs::read_dir(&day_path)? {
                    let file_entry = file_entry?;
                    let file_path = file_entry.path();

                    if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                            files.push(file_path);
                        }
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

    /// Creates a temporary JSONL file with given lines.
    fn create_temp_session_file(lines: &[&str]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        for line in lines {
            writeln!(file, "{line}").expect("Failed to write line");
        }
        file.flush().expect("Failed to flush");
        file
    }

    /// Generate a session_meta line.
    fn make_session_meta(session_id: &str, cwd: &str, version: &str) -> String {
        format!(
            r#"{{"timestamp":"2025-12-18T22:53:29.406Z","type":"session_meta","payload":{{"id":"{session_id}","timestamp":"2025-12-18T22:53:29.377Z","cwd":"{cwd}","originator":"codex_cli_rs","cli_version":"{version}","model_provider":"openai","git":{{"branch":"main"}}}}}}"#
        )
    }

    /// Generate a user response_item line.
    fn make_user_message(content: &str) -> String {
        format!(
            r#"{{"timestamp":"2025-12-18T22:54:00.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{content}"}}]}}}}"#
        )
    }

    /// Generate an assistant response_item line.
    fn make_assistant_message(content: &str) -> String {
        format!(
            r#"{{"timestamp":"2025-12-18T22:55:00.000Z","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"text","text":"{content}"}}]}}}}"#
        )
    }

    // Note: Common watcher trait tests (info, watch_paths, find_sources) are in
    // src/capture/watchers/test_common.rs to avoid duplication across all watchers.
    // Only tool-specific parsing tests remain here.

    #[test]
    fn test_parse_session_meta() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/Users/test/project", "0.63.0");

        let file = create_temp_session_file(&[&meta_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.session_id, session_id);
        assert_eq!(parsed.cli_version, Some("0.63.0".to_string()));
        assert_eq!(parsed.cwd, "/Users/test/project");
        assert_eq!(parsed.model_provider, Some("openai".to_string()));
        assert_eq!(parsed.git_branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_user_message() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let user_line = make_user_message("Hello, can you help me?");

        let file = create_temp_session_file(&[&meta_line, &user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "Hello, can you help me?");
    }

    #[test]
    fn test_parse_assistant_message() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let assistant_line = make_assistant_message("Sure, I can help!");

        let file = create_temp_session_file(&[&meta_line, &assistant_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[0].content, "Sure, I can help!");
    }

    #[test]
    fn test_parse_conversation() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let user_line = make_user_message("Hello");
        let assistant_line = make_assistant_message("Hi there!");

        let file = create_temp_session_file(&[&meta_line, &user_line, &assistant_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_to_storage_models() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test/project", "0.63.0");
        let user_line = make_user_message("Hello");
        let assistant_line = make_assistant_message("Hi!");

        let file = create_temp_session_file(&[&meta_line, &user_line, &assistant_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");
        let (session, messages) = parsed.to_storage_models();

        assert_eq!(session.tool, "codex");
        assert_eq!(session.tool_version, Some("0.63.0".to_string()));
        assert_eq!(session.working_directory, "/test/project");
        assert_eq!(session.git_branch, Some("main".to_string()));
        assert_eq!(session.model, Some("openai".to_string()));
        assert_eq!(session.message_count, 2);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[0].index, 0);
        assert_eq!(messages[1].index, 1);
    }

    #[test]
    fn test_empty_lines_skipped() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let user_line = make_user_message("Hello");

        let file = create_temp_session_file(&["", &meta_line, "  ", &user_line, ""]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
    }

    #[test]
    fn test_invalid_json_skipped() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let user_line = make_user_message("Hello");

        let file =
            create_temp_session_file(&["invalid json", &meta_line, "{not valid", &user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.session_id, session_id);
    }

    #[test]
    fn test_non_message_response_items_skipped() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        // A response_item with type other than "message"
        let other_item = r#"{"timestamp":"2025-12-18T22:54:00.000Z","type":"response_item","payload":{"type":"function_call","name":"test"}}"#;
        let user_line = make_user_message("Hello");

        let file = create_temp_session_file(&[&meta_line, other_item, &user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
    }

    #[test]
    fn test_empty_content_skipped() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let empty_content = r#"{"timestamp":"2025-12-18T22:54:00.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[]}}"#;
        let user_line = make_user_message("Hello");

        let file = create_temp_session_file(&[&meta_line, empty_content, &user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        assert_eq!(parsed.messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = CodexWatcher;
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let user_line = make_user_message("Hello");

        let file = create_temp_session_file(&[&meta_line, &user_line]);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert_eq!(result.len(), 1);
        let (session, messages) = &result[0];
        assert_eq!(session.tool, "codex");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_watcher_parse_source_empty_session() {
        let watcher = CodexWatcher;
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");

        // Only metadata, no messages
        let file = create_temp_session_file(&[&meta_line]);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(result.is_empty());
    }

    #[test]
    fn test_session_id_fallback_to_filename() {
        // File with no session_meta
        let user_line = make_user_message("Hello");
        let file = create_temp_session_file(&[&user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");

        // Should fall back to filename
        assert!(!parsed.session_id.is_empty());
    }

    #[test]
    fn test_uuid_session_id_parsing() {
        let session_id = "019b33ab-179f-7802-88a6-16557b4b7603";
        let meta_line = make_session_meta(session_id, "/test", "0.63.0");
        let user_line = make_user_message("Hello");

        let file = create_temp_session_file(&[&meta_line, &user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");
        let (session, _) = parsed.to_storage_models();

        // The session ID should be parsed as a valid UUID
        assert_eq!(session.id.to_string(), session_id);
    }

    #[test]
    fn test_invalid_uuid_generates_new() {
        let meta_line = r#"{"timestamp":"2025-12-18T22:53:29.406Z","type":"session_meta","payload":{"id":"not-a-uuid","timestamp":"2025-12-18T22:53:29.377Z","cwd":"/test","cli_version":"0.63.0"}}"#;
        let user_line = make_user_message("Hello");

        let file = create_temp_session_file(&[meta_line, &user_line]);
        let parsed = parse_codex_session_file(file.path()).expect("Failed to parse");
        let (session, _) = parsed.to_storage_models();

        // Should still have a valid UUID (newly generated)
        assert!(!session.id.is_nil());
    }
}

//! Aider session parser.
//!
//! Parses chat history from Aider's markdown files. Aider stores conversation
//! history in `.aider.chat.history.md` files in project directories.
//!
//! The format uses level 4 headings (`####`) for user messages, with assistant
//! responses following as regular markdown text. Tool outputs are prefixed with
//! `>` blockquotes.
//!
//! By default, Aider stores history in the project's root directory as
//! `.aider.chat.history.md`. Users can configure a different location using
//! the `--chat-history-file` option or `AIDER_CHAT_HISTORY_FILE` environment
//! variable.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::storage::models::{Message, MessageContent, MessageRole, Session};

use super::{Watcher, WatcherInfo};

/// Watcher for Aider sessions.
///
/// Discovers and parses `.aider.chat.history.md` files from project directories.
/// Aider is a terminal-based AI coding assistant that stores conversation
/// history in markdown format.
pub struct AiderWatcher;

impl Watcher for AiderWatcher {
    fn info(&self) -> WatcherInfo {
        WatcherInfo {
            name: "aider",
            description: "Aider terminal AI chat sessions",
            default_paths: vec![],
        }
    }

    fn is_available(&self) -> bool {
        // Aider stores files in project directories, not a central location.
        // We check if the aider command exists or if there are any history files
        // in common locations. For now, we always return true since files can
        // exist anywhere.
        true
    }

    fn find_sources(&self) -> Result<Vec<PathBuf>> {
        find_aider_history_files()
    }

    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
        let parsed = parse_aider_history(path)?;
        if parsed.is_empty() {
            return Ok(vec![]);
        }
        Ok(parsed)
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        // Aider files are in project directories, not a central location.
        // Return home directory as a broad watch target.
        if let Some(home) = dirs::home_dir() {
            vec![home]
        } else {
            vec![]
        }
    }
}

/// Finds Aider history files in common locations.
///
/// Searches the home directory and common project locations for
/// `.aider.chat.history.md` files. This is a best-effort search since
/// Aider files can be in any project directory.
fn find_aider_history_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    // Check home directory
    if let Some(home) = dirs::home_dir() {
        let home_history = home.join(".aider.chat.history.md");
        if home_history.exists() {
            files.push(home_history);
        }

        // Check common project directories
        for dir_name in &["projects", "code", "src", "dev", "workspace", "repos"] {
            let dir = home.join(dir_name);
            if dir.exists() {
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let history_file = entry.path().join(".aider.chat.history.md");
                        if history_file.exists() {
                            files.push(history_file);
                        }
                    }
                }
            }
        }
    }

    Ok(files)
}

/// Parses an Aider chat history markdown file.
///
/// The format consists of:
/// - `####` headings for user messages
/// - Regular text for assistant responses
/// - `>` blockquotes for tool output
///
/// Each contiguous conversation (no blank lines between user/assistant) is
/// treated as a session.
fn parse_aider_history(path: &Path) -> Result<Vec<(Session, Vec<Message>)>> {
    let content = fs::read_to_string(path).context("Failed to read Aider history file")?;

    let working_directory = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let mut sessions = Vec::new();
    let mut current_messages: Vec<ParsedMessage> = Vec::new();
    let mut current_role: Option<MessageRole> = None;
    let mut current_content = String::new();
    let mut in_tool_output = false;

    for line in content.lines() {
        // User message starts with ####
        if line.starts_with("#### ") {
            // Save any pending content
            if let Some(role) = current_role.take() {
                if !current_content.trim().is_empty() {
                    current_messages.push(ParsedMessage {
                        role,
                        content: current_content.trim().to_string(),
                    });
                }
            }

            // Start new user message
            current_role = Some(MessageRole::User);
            current_content = line.strip_prefix("#### ").unwrap_or("").to_string();
            in_tool_output = false;
        }
        // Tool output (blockquote)
        else if line.starts_with("> ") || line == ">" {
            // Tool output is part of assistant response
            if current_role == Some(MessageRole::User) && !current_content.trim().is_empty() {
                // Save user message first
                current_messages.push(ParsedMessage {
                    role: MessageRole::User,
                    content: current_content.trim().to_string(),
                });
                current_content.clear();
                current_role = Some(MessageRole::Assistant);
            } else if current_role.is_none() {
                current_role = Some(MessageRole::Assistant);
            }

            in_tool_output = true;
            let tool_line = line
                .strip_prefix("> ")
                .unwrap_or(line.strip_prefix(">").unwrap_or(""));
            if !current_content.is_empty() {
                current_content.push('\n');
            }
            current_content.push_str(tool_line);
        }
        // Blank line might indicate end of message or section
        else if line.trim().is_empty() {
            if in_tool_output {
                // End of tool output block
                in_tool_output = false;
                if !current_content.is_empty() {
                    current_content.push('\n');
                }
            } else if current_role == Some(MessageRole::User) && !current_content.trim().is_empty()
            {
                // End of user message, switch to assistant
                current_messages.push(ParsedMessage {
                    role: MessageRole::User,
                    content: current_content.trim().to_string(),
                });
                current_content.clear();
                current_role = Some(MessageRole::Assistant);
            } else if current_role == Some(MessageRole::Assistant) {
                // Blank line in assistant content
                if !current_content.is_empty() {
                    current_content.push('\n');
                }
            }
        }
        // Regular line (assistant response or continuation)
        else {
            if current_role.is_none() {
                // Orphan content before any user message - treat as assistant
                current_role = Some(MessageRole::Assistant);
            } else if current_role == Some(MessageRole::User) && !line.starts_with("####") {
                // This line follows user input - could be continuation or assistant response
                // In Aider format, assistant responses directly follow user messages
                if !current_content.trim().is_empty() {
                    current_messages.push(ParsedMessage {
                        role: MessageRole::User,
                        content: current_content.trim().to_string(),
                    });
                    current_content.clear();
                    current_role = Some(MessageRole::Assistant);
                }
            }

            if !current_content.is_empty() {
                current_content.push('\n');
            }
            current_content.push_str(line);
        }
    }

    // Save any remaining content
    if let Some(role) = current_role {
        if !current_content.trim().is_empty() {
            current_messages.push(ParsedMessage {
                role,
                content: current_content.trim().to_string(),
            });
        }
    }

    // Convert parsed messages to session
    if !current_messages.is_empty() {
        let session = create_session(path, &working_directory, current_messages.len());
        let messages = create_messages(&session, &current_messages);
        sessions.push((session, messages));
    }

    Ok(sessions)
}

/// A parsed message from Aider history.
struct ParsedMessage {
    role: MessageRole,
    content: String,
}

/// Creates a Session from parsed Aider history.
fn create_session(path: &Path, working_directory: &str, message_count: usize) -> Session {
    // Use file modification time as session end time
    let ended_at = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from);

    // Estimate start time as a bit before end time based on message count
    let started_at = ended_at
        .map(|t| t - chrono::Duration::minutes(message_count as i64 * 2))
        .unwrap_or_else(Utc::now);

    Session {
        id: Uuid::new_v4(),
        tool: "aider".to_string(),
        tool_version: None,
        started_at,
        ended_at,
        model: None,
        working_directory: working_directory.to_string(),
        git_branch: None,
        source_path: Some(path.to_string_lossy().to_string()),
        message_count: message_count as i32,
        machine_id: crate::storage::get_machine_id(),
    }
}

/// Creates Messages from parsed Aider content.
fn create_messages(session: &Session, parsed_messages: &[ParsedMessage]) -> Vec<Message> {
    let time_per_message = chrono::Duration::seconds(30);
    let mut current_time = session.started_at;

    parsed_messages
        .iter()
        .enumerate()
        .map(|(idx, msg)| {
            let message = Message {
                id: Uuid::new_v4(),
                session_id: session.id,
                parent_id: None,
                index: idx as i32,
                timestamp: current_time,
                role: msg.role.clone(),
                content: MessageContent::Text(msg.content.clone()),
                model: None,
                git_branch: None,
                cwd: Some(session.working_directory.clone()),
            };
            current_time += time_per_message;
            message
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Creates a temporary Aider history file with given content.
    fn create_temp_history_file(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        file.write_all(content.as_bytes())
            .expect("Failed to write content");
        file.flush().expect("Failed to flush");
        file
    }

    #[test]
    fn test_watcher_info() {
        let watcher = AiderWatcher;
        let info = watcher.info();

        assert_eq!(info.name, "aider");
        assert_eq!(info.description, "Aider terminal AI chat sessions");
    }

    #[test]
    fn test_watcher_is_available() {
        let watcher = AiderWatcher;
        // Aider watcher is always available since files can be anywhere
        assert!(watcher.is_available());
    }

    #[test]
    fn test_parse_simple_conversation() {
        let content = r#"#### Hello, can you help me with a Rust project?

Sure! I'd be happy to help you with your Rust project. What would you like to do?

#### Can you create a simple function?

Here's a simple function:

```rust
fn hello() {
    println!("Hello, world!");
}
```
"#;

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        assert_eq!(result.len(), 1);
        let (session, messages) = &result[0];
        assert_eq!(session.tool, "aider");
        assert!(messages.len() >= 2);
    }

    #[test]
    fn test_parse_with_tool_output() {
        let content = r#"#### Run the tests

> Running tests...
> test result: ok. 5 passed; 0 failed

All tests passed successfully!
"#;

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        assert_eq!(result.len(), 1);
        let (_, messages) = &result[0];
        assert!(!messages.is_empty());
    }

    #[test]
    fn test_parse_empty_file() {
        let content = "";

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_user_message_only() {
        let content = "#### What is Rust?\n";

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        assert_eq!(result.len(), 1);
        let (_, messages) = &result[0];
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::User);
    }

    #[test]
    fn test_parse_multiple_exchanges() {
        let content = r#"#### First question

First answer

#### Second question

Second answer

#### Third question

Third answer
"#;

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        assert_eq!(result.len(), 1);
        let (_, messages) = &result[0];
        // Should have 3 user messages and 3 assistant messages
        assert!(messages.len() >= 3);
    }

    #[test]
    fn test_session_metadata() {
        let content = "#### Test message\n\nTest response\n";

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        let (session, _) = &result[0];
        assert_eq!(session.tool, "aider");
        assert!(session.source_path.is_some());
        assert!(session.ended_at.is_some());
    }

    #[test]
    fn test_find_aider_history_files_returns_ok() {
        // Should not error even if no files exist
        let result = find_aider_history_files();
        assert!(result.is_ok());
    }

    #[test]
    fn test_watcher_parse_source() {
        let watcher = AiderWatcher;
        let content = "#### Test\n\nResponse\n";

        let file = create_temp_history_file(content);
        let result = watcher
            .parse_source(file.path())
            .expect("Should parse successfully");

        assert!(!result.is_empty());
        let (session, _) = &result[0];
        assert_eq!(session.tool, "aider");
    }

    #[test]
    fn test_message_roles_alternate() {
        let content = r#"#### User message 1

Assistant response 1

#### User message 2

Assistant response 2
"#;

        let file = create_temp_history_file(content);
        let result = parse_aider_history(file.path()).expect("Should parse");

        let (_, messages) = &result[0];
        assert!(messages.len() >= 2);

        // Check that roles alternate properly
        for (i, msg) in messages.iter().enumerate() {
            if i % 2 == 0 {
                assert_eq!(msg.role, MessageRole::User);
            } else {
                assert_eq!(msg.role, MessageRole::Assistant);
            }
        }
    }
}

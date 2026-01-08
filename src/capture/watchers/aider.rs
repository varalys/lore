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
        // Check if aider command exists or if any history files exist
        if std::process::Command::new("aider")
            .arg("--version")
            .output()
            .is_ok()
        {
            return true;
        }

        // Fall back to checking if any history files exist
        find_aider_history_files()
            .map(|files| !files.is_empty())
            .unwrap_or(false)
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
        // Aider stores .aider.chat.history.md files in individual project directories,
        // not in a central location. Watching the entire home directory is impractical
        // (too many files, exceeds inotify limits, high memory usage).
        //
        // Instead, aider sessions are only captured via manual `lore import`.
        // Real-time watching is not supported for aider.
        vec![]
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

/// Directories that should be skipped when scanning for aider files.
///
/// These are typically hidden directories, build artifacts, or cache directories
/// that are unlikely to contain project files and would slow down scanning.
const SKIP_DIRS: &[&str] = &[
    // Hidden directories (general)
    ".git",
    ".svn",
    ".hg",
    // Build artifacts and dependencies
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    "venv",
    ".venv",
    "env",
    ".env",
    ".tox",
    ".nox",
    // Package managers
    ".npm",
    ".yarn",
    ".pnpm",
    ".cargo",
    ".rustup",
    // IDE and editor directories
    ".idea",
    ".vscode",
    ".eclipse",
    // Cache and temp directories
    ".cache",
    ".local",
    ".config",
    ".Trash",
    // macOS
    "Library",
    // Other
    "vendor",
    ".bundle",
];

/// Directories that should not be skipped even if they start with a dot.
///
/// These are known tool directories that may contain useful session data.
const ALLOW_HIDDEN_DIRS: &[&str] = &[".claude", ".continue", ".codex", ".amp"];

/// Scans directories recursively for aider history files.
///
/// This function searches the given directories for `.aider.chat.history.md` files,
/// skipping hidden directories and common build artifact locations for efficiency.
///
/// # Arguments
/// * `directories` - List of directories to scan
/// * `progress_callback` - Called with (current_dir, files_found_so_far) during scanning
///
/// # Returns
/// A vector of paths to discovered aider history files.
pub fn scan_directories_for_aider_files<F>(
    directories: &[PathBuf],
    mut progress_callback: F,
) -> Vec<PathBuf>
where
    F: FnMut(&Path, usize),
{
    let mut found_files = Vec::new();

    for dir in directories {
        if dir.exists() && dir.is_dir() {
            scan_directory_recursive(dir, &mut found_files, &mut progress_callback);
        }
    }

    found_files
}

/// Recursively scans a directory for aider history files.
fn scan_directory_recursive<F>(
    dir: &Path,
    found_files: &mut Vec<PathBuf>,
    progress_callback: &mut F,
) where
    F: FnMut(&Path, usize),
{
    // Report progress
    progress_callback(dir, found_files.len());

    // Check for aider history file in this directory
    let history_file = dir.join(".aider.chat.history.md");
    if history_file.exists() {
        found_files.push(history_file);
    }

    // Read directory entries
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // Skip directories we can't read
    };

    // Recurse into subdirectories
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Skip directories in the skip list
        if SKIP_DIRS.contains(&dir_name) {
            continue;
        }

        // Skip hidden directories unless they're in the allow list
        if dir_name.starts_with('.') && !ALLOW_HIDDEN_DIRS.contains(&dir_name) {
            continue;
        }

        // Recurse
        scan_directory_recursive(&path, found_files, progress_callback);
    }
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
        // is_available returns true if aider command exists or history files found
        // We just verify the method runs without panicking
        let _ = watcher.is_available();
    }

    #[test]
    fn test_watcher_watch_paths_returns_empty() {
        let watcher = AiderWatcher;
        // Aider files are scattered across project directories, so we don't watch
        // any paths in real-time (would require watching entire home directory)
        let paths = watcher.watch_paths();
        assert!(
            paths.is_empty(),
            "watch_paths should return empty vec for aider"
        );
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

    #[test]
    fn test_scan_directories_finds_aider_files() {
        use tempfile::TempDir;

        // Create a temp directory structure with an aider file
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let project_dir = temp_dir.path().join("my-project");
        std::fs::create_dir(&project_dir).expect("Failed to create project dir");

        // Create an aider history file
        let history_file = project_dir.join(".aider.chat.history.md");
        std::fs::write(&history_file, "#### Test\n\nResponse\n").expect("Failed to write file");

        // Scan the directory
        let mut progress_calls = 0;
        let found = scan_directories_for_aider_files(&[temp_dir.path().to_path_buf()], |_, _| {
            progress_calls += 1;
        });

        assert_eq!(found.len(), 1);
        assert_eq!(found[0], history_file);
        assert!(progress_calls > 0, "Progress callback should be called");
    }

    #[test]
    fn test_scan_directories_skips_hidden_dirs() {
        use tempfile::TempDir;

        // Create a temp directory with a hidden directory containing an aider file
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let hidden_dir = temp_dir.path().join(".hidden-project");
        std::fs::create_dir(&hidden_dir).expect("Failed to create hidden dir");

        // Create an aider history file in the hidden directory
        let history_file = hidden_dir.join(".aider.chat.history.md");
        std::fs::write(&history_file, "#### Test\n\nResponse\n").expect("Failed to write file");

        // Scan the directory - should NOT find the file in hidden dir
        let found = scan_directories_for_aider_files(&[temp_dir.path().to_path_buf()], |_, _| {});

        assert!(
            found.is_empty(),
            "Should not find files in hidden directories"
        );
    }

    #[test]
    fn test_scan_directories_skips_node_modules() {
        use tempfile::TempDir;

        // Create a temp directory with node_modules containing an aider file
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let node_modules = temp_dir.path().join("node_modules").join("some-package");
        std::fs::create_dir_all(&node_modules).expect("Failed to create node_modules");

        // Create an aider history file in node_modules
        let history_file = node_modules.join(".aider.chat.history.md");
        std::fs::write(&history_file, "#### Test\n\nResponse\n").expect("Failed to write file");

        // Scan the directory - should NOT find the file
        let found = scan_directories_for_aider_files(&[temp_dir.path().to_path_buf()], |_, _| {});

        assert!(found.is_empty(), "Should not find files in node_modules");
    }

    #[test]
    fn test_scan_directories_empty_input() {
        let found = scan_directories_for_aider_files(&[], |_, _| {});
        assert!(found.is_empty());
    }
}

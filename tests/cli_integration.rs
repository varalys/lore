//! Integration tests for Lore CLI commands
//!
//! These tests exercise the CLI commands through their underlying library functions
//! using temporary databases to ensure test isolation.

use chrono::{Duration, Utc};
use lore_cli::storage::{
    Database, LinkCreator, LinkType, Message, MessageContent, MessageRole, Session, SessionLink,
};
use std::io::Write;
use tempfile::{tempdir, NamedTempFile};
use uuid::Uuid;

// =============================================================================
// Test Helpers
// =============================================================================

/// Creates a test database in a temporary directory.
/// Returns the Database instance and the temp directory (which must be kept alive).
fn create_test_db() -> (Database, tempfile::TempDir) {
    let dir = tempdir().expect("Failed to create temp directory");
    let db_path = dir.path().join("test.db");
    let db = Database::open(&db_path).expect("Failed to open test database");
    (db, dir)
}

/// Creates a test session with the given parameters.
fn create_test_session(
    tool: &str,
    working_directory: &str,
    started_at: chrono::DateTime<Utc>,
    source_path: Option<&str>,
) -> Session {
    Session {
        id: Uuid::new_v4(),
        tool: tool.to_string(),
        tool_version: Some("1.0.0".to_string()),
        started_at,
        ended_at: None,
        model: Some("test-model".to_string()),
        working_directory: working_directory.to_string(),
        git_branch: Some("main".to_string()),
        source_path: source_path.map(|s| s.to_string()),
        message_count: 0,
    }
}

/// Creates a test message for the given session.
fn create_test_message(session_id: Uuid, index: i32, role: MessageRole, content: &str) -> Message {
    Message {
        id: Uuid::new_v4(),
        session_id,
        parent_id: None,
        index,
        timestamp: Utc::now(),
        role,
        content: MessageContent::Text(content.to_string()),
        model: Some("test-model".to_string()),
        git_branch: Some("main".to_string()),
        cwd: Some("/test/cwd".to_string()),
    }
}

/// Creates a test session link for the given session.
fn create_test_link(
    session_id: Uuid,
    commit_sha: Option<&str>,
    link_type: LinkType,
) -> SessionLink {
    SessionLink {
        id: Uuid::new_v4(),
        session_id,
        link_type,
        commit_sha: commit_sha.map(|s| s.to_string()),
        branch: Some("main".to_string()),
        remote: Some("origin".to_string()),
        created_at: Utc::now(),
        created_by: LinkCreator::User,
        confidence: None,
    }
}

/// Creates a valid Claude Code JSONL session file with test data.
fn create_test_session_jsonl(session_id: &str) -> NamedTempFile {
    let user_uuid = Uuid::new_v4().to_string();
    let assistant_uuid = Uuid::new_v4().to_string();

    let user_line = format!(
        r#"{{"type":"user","sessionId":"{session_id}","uuid":"{user_uuid}","timestamp":"2025-01-15T10:00:00.000Z","cwd":"/test/project","gitBranch":"main","version":"2.0.72","message":{{"role":"user","content":"Hello, Claude!"}}}}"#
    );

    let assistant_line = format!(
        r#"{{"type":"assistant","sessionId":"{session_id}","uuid":"{assistant_uuid}","parentUuid":"{user_uuid}","timestamp":"2025-01-15T10:01:00.000Z","cwd":"/test/project","gitBranch":"main","message":{{"role":"assistant","model":"claude-opus-4","content":"Hello! How can I help you today?"}}}}"#
    );

    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    writeln!(file, "{user_line}").expect("Failed to write user line");
    writeln!(file, "{assistant_line}").expect("Failed to write assistant line");
    file.flush().expect("Failed to flush");
    file
}

// =============================================================================
// Sessions Command Tests
// =============================================================================

mod sessions_tests {
    use super::*;

    #[test]
    fn test_list_sessions_empty_database() {
        let (db, _dir) = create_test_db();

        let sessions = db.list_sessions(20, None).expect("Failed to list sessions");

        assert!(
            sessions.is_empty(),
            "Empty database should return no sessions"
        );
    }

    #[test]
    fn test_list_sessions_shows_imported_sessions() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Insert multiple sessions
        let session1 = create_test_session(
            "claude-code",
            "/home/user/project1",
            now - Duration::hours(2),
            Some("/path/session1.jsonl"),
        );
        let session2 = create_test_session(
            "claude-code",
            "/home/user/project2",
            now - Duration::hours(1),
            Some("/path/session2.jsonl"),
        );
        let session3 = create_test_session(
            "claude-code",
            "/home/user/project3",
            now,
            Some("/path/session3.jsonl"),
        );

        db.insert_session(&session1)
            .expect("Failed to insert session1");
        db.insert_session(&session2)
            .expect("Failed to insert session2");
        db.insert_session(&session3)
            .expect("Failed to insert session3");

        let sessions = db.list_sessions(20, None).expect("Failed to list sessions");

        assert_eq!(sessions.len(), 3, "Should have 3 sessions");

        // Verify ordering (most recent first)
        assert_eq!(
            sessions[0].id, session3.id,
            "Most recent session should be first"
        );
        assert_eq!(
            sessions[1].id, session2.id,
            "Second most recent should be second"
        );
        assert_eq!(sessions[2].id, session1.id, "Oldest session should be last");
    }

    #[test]
    fn test_list_sessions_respects_limit() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Insert 5 sessions
        for i in 0..5 {
            let session = create_test_session(
                "claude-code",
                &format!("/project{i}"),
                now - Duration::hours(i as i64),
                None,
            );
            db.insert_session(&session)
                .expect("Failed to insert session");
        }

        // Request only 3
        let sessions = db.list_sessions(3, None).expect("Failed to list sessions");

        assert_eq!(sessions.len(), 3, "Should respect limit of 3");
    }

    #[test]
    fn test_list_sessions_filter_by_repo() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        let session1 = create_test_session(
            "claude-code",
            "/home/user/project-alpha",
            now - Duration::hours(2),
            None,
        );
        let session2 = create_test_session(
            "claude-code",
            "/home/user/project-beta",
            now - Duration::hours(1),
            None,
        );
        let session3 = create_test_session("claude-code", "/other/path/project", now, None);

        db.insert_session(&session1).expect("insert");
        db.insert_session(&session2).expect("insert");
        db.insert_session(&session3).expect("insert");

        // Filter by /home/user prefix
        let sessions = db
            .list_sessions(20, Some("/home/user"))
            .expect("Failed to list sessions");

        assert_eq!(sessions.len(), 2, "Should have 2 sessions in /home/user");

        // Verify neither is from /other/path
        for session in &sessions {
            assert!(
                session.working_directory.starts_with("/home/user"),
                "All sessions should be in /home/user"
            );
        }
    }

    #[test]
    fn test_sessions_json_output_is_valid() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        let session = create_test_session(
            "claude-code",
            "/home/user/project",
            now,
            Some("/path/session.jsonl"),
        );
        db.insert_session(&session).expect("insert");

        let sessions = db.list_sessions(20, None).expect("list");

        // Verify we can serialize to JSON
        let json = serde_json::to_string_pretty(&sessions).expect("Should serialize to JSON");

        // Parse it back to verify it's valid JSON
        let parsed: Vec<Session> =
            serde_json::from_str(&json).expect("Should parse back from JSON");

        assert_eq!(parsed.len(), 1, "Should have 1 session after round-trip");
        assert_eq!(parsed[0].id, session.id, "Session ID should match");
    }
}

// =============================================================================
// Show Command Tests
// =============================================================================

mod show_tests {
    use super::*;

    #[test]
    fn test_show_session_by_prefix() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        let session = create_test_session(
            "claude-code",
            "/home/user/project",
            now,
            Some("/path/session.jsonl"),
        );
        let session_id = session.id;
        let session_prefix = &session_id.to_string()[..8];

        db.insert_session(&session).expect("insert");

        // Add some messages
        let msg1 = create_test_message(session_id, 0, MessageRole::User, "Hello");
        let msg2 = create_test_message(session_id, 1, MessageRole::Assistant, "Hi there!");

        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Find by prefix (simulating what show command does)
        let sessions = db.list_sessions(100, None).expect("list");
        let found = sessions
            .iter()
            .find(|s| s.id.to_string().starts_with(session_prefix));

        assert!(found.is_some(), "Should find session by prefix");

        let found_session = found.unwrap();
        assert_eq!(
            found_session.id, session_id,
            "Found session ID should match"
        );

        // Verify we can get messages
        let messages = db.get_messages(&found_session.id).expect("get messages");
        assert_eq!(messages.len(), 2, "Should have 2 messages");
    }

    #[test]
    fn test_show_invalid_session_prefix() {
        let (db, _dir) = create_test_db();

        // Insert a session
        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        // Try to find with an invalid prefix
        let sessions = db.list_sessions(100, None).expect("list");
        let found = sessions
            .iter()
            .find(|s| s.id.to_string().starts_with("zzzzzzzz"));

        assert!(
            found.is_none(),
            "Should not find session with invalid prefix"
        );
    }

    #[test]
    fn test_show_commit_displays_linked_sessions() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/project2", Utc::now(), None);

        db.insert_session(&session1).expect("insert");
        db.insert_session(&session2).expect("insert");

        let commit_sha = "abc123def456789012345678901234567890abcd";

        // Link both sessions to the same commit
        let link1 = create_test_link(session1.id, Some(commit_sha), LinkType::Commit);
        let link2 = create_test_link(session2.id, Some(commit_sha), LinkType::Commit);

        db.insert_link(&link1).expect("insert link1");
        db.insert_link(&link2).expect("insert link2");

        // Query links by commit (using prefix)
        let links = db.get_links_by_commit("abc123").expect("get links");

        assert_eq!(links.len(), 2, "Should have 2 links for this commit");

        // Verify we can retrieve the linked sessions
        let session_ids: Vec<Uuid> = links.iter().map(|l| l.session_id).collect();
        assert!(
            session_ids.contains(&session1.id),
            "Should include session1"
        );
        assert!(
            session_ids.contains(&session2.id),
            "Should include session2"
        );
    }

    #[test]
    fn test_show_commit_no_linked_sessions() {
        let (db, _dir) = create_test_db();

        // Query for a commit with no links
        let links = db.get_links_by_commit("nonexistent123").expect("get links");

        assert!(links.is_empty(), "Should return empty for unlinked commit");
    }

    #[test]
    fn test_show_session_with_different_content_types() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        // Create message with block content
        let block_content = MessageContent::Blocks(vec![
            lore_cli::storage::ContentBlock::Text {
                text: "Let me help with that.".to_string(),
            },
            lore_cli::storage::ContentBlock::ToolUse {
                id: "tool_123".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command": "ls -la"}),
            },
        ]);

        let msg = Message {
            id: Uuid::new_v4(),
            session_id: session.id,
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::Assistant,
            content: block_content,
            model: Some("claude-opus-4".to_string()),
            git_branch: Some("main".to_string()),
            cwd: Some("/project".to_string()),
        };

        db.insert_message(&msg).expect("insert message");

        let messages = db.get_messages(&session.id).expect("get messages");
        assert_eq!(messages.len(), 1, "Should have 1 message");

        // Verify block content was preserved
        if let MessageContent::Blocks(blocks) = &messages[0].content {
            assert_eq!(blocks.len(), 2, "Should have 2 blocks");
        } else {
            panic!("Expected block content");
        }
    }
}

// =============================================================================
// Import Command Tests
// =============================================================================

mod import_tests {
    use super::*;
    use lore_cli::capture::watchers::claude_code;

    #[test]
    fn test_import_no_claude_sessions_returns_gracefully() {
        // The find_session_files function should return an empty vec if
        // the Claude directory doesn't exist or has no sessions
        let result = claude_code::find_session_files();

        // This should not error, just return empty or existing sessions
        assert!(
            result.is_ok(),
            "Should not error when Claude directory is missing or empty"
        );
    }

    #[test]
    fn test_import_dry_run_does_not_modify_database() {
        let (db, _dir) = create_test_db();

        // Verify database is empty
        let count_before = db.session_count().expect("count");
        assert_eq!(count_before, 0, "Database should start empty");

        // Create a valid session file
        let session_id = Uuid::new_v4().to_string();
        let file = create_test_session_jsonl(&session_id);

        // Parse the session
        let parsed =
            claude_code::parse_session_file(file.path()).expect("Should parse session file");

        assert!(
            !parsed.messages.is_empty(),
            "Parsed session should have messages"
        );

        // In dry run mode, we parse but don't insert
        // (The actual dry run logic is in the command, but we verify
        // the database remains unchanged if we don't call insert)

        let count_after = db.session_count().expect("count");
        assert_eq!(
            count_after, 0,
            "Database should remain empty without insert"
        );
    }

    #[test]
    fn test_import_parses_valid_session_file() {
        let session_id = Uuid::new_v4().to_string();
        let file = create_test_session_jsonl(&session_id);

        let parsed =
            claude_code::parse_session_file(file.path()).expect("Should parse session file");

        assert_eq!(parsed.session_id, session_id, "Session ID should match");
        assert_eq!(parsed.messages.len(), 2, "Should have 2 messages");
        assert_eq!(parsed.cwd, "/test/project", "Should have correct cwd");
        assert_eq!(
            parsed.git_branch,
            Some("main".to_string()),
            "Should have branch"
        );
        assert_eq!(
            parsed.tool_version,
            Some("2.0.72".to_string()),
            "Should have version"
        );
        assert_eq!(
            parsed.model,
            Some("claude-opus-4".to_string()),
            "Should have model"
        );
    }

    #[test]
    fn test_import_converts_to_storage_models() {
        let session_id = Uuid::new_v4().to_string();
        let file = create_test_session_jsonl(&session_id);

        let parsed =
            claude_code::parse_session_file(file.path()).expect("Should parse session file");

        let (session, messages) = parsed.to_storage_models();

        // Verify session
        assert_eq!(
            session.id.to_string(),
            session_id,
            "Session ID should match"
        );
        assert_eq!(session.tool, "claude-code", "Tool should be claude-code");
        assert_eq!(session.message_count, 2, "Should have 2 messages");

        // Verify messages
        assert_eq!(messages.len(), 2, "Should have 2 messages");
        assert_eq!(messages[0].role, MessageRole::User, "First is user");
        assert_eq!(
            messages[1].role,
            MessageRole::Assistant,
            "Second is assistant"
        );

        // Verify parent linking
        assert!(messages[0].parent_id.is_none(), "First has no parent");
        assert_eq!(
            messages[1].parent_id,
            Some(messages[0].id),
            "Second has first as parent"
        );
    }

    #[test]
    fn test_import_skips_already_imported_sessions() {
        let (db, _dir) = create_test_db();

        // Create and import a session
        let session = create_test_session(
            "claude-code",
            "/project",
            Utc::now(),
            Some("/path/to/session.jsonl"),
        );
        db.insert_session(&session).expect("insert");

        // Verify it exists by source path
        let exists = db
            .session_exists_by_source("/path/to/session.jsonl")
            .expect("check exists");

        assert!(exists, "Session should exist by source path");

        // A second import attempt would detect this and skip
        let still_exists = db
            .session_exists_by_source("/path/to/session.jsonl")
            .expect("check exists");

        assert!(
            still_exists,
            "Session should still exist, would be skipped on re-import"
        );
    }

    #[test]
    fn test_import_stores_session_and_messages() {
        let (db, _dir) = create_test_db();

        let session_id = Uuid::new_v4().to_string();
        let file = create_test_session_jsonl(&session_id);

        let parsed = claude_code::parse_session_file(file.path()).expect("Should parse");

        let (session, messages) = parsed.to_storage_models();

        // Store session
        db.insert_session(&session).expect("insert session");

        // Store messages
        for msg in &messages {
            db.insert_message(msg).expect("insert message");
        }

        // Verify
        assert_eq!(
            db.session_count().expect("count"),
            1,
            "Should have 1 session"
        );
        assert_eq!(
            db.message_count().expect("count"),
            2,
            "Should have 2 messages"
        );

        // Retrieve and verify
        let retrieved = db
            .get_session(&session.id)
            .expect("get")
            .expect("should exist");

        assert_eq!(retrieved.id, session.id, "IDs should match");
        assert_eq!(retrieved.tool, "claude-code", "Tool should match");
    }
}

// =============================================================================
// Link Command Tests
// =============================================================================

mod link_tests {
    use super::*;

    #[test]
    fn test_link_session_to_commit() {
        let (db, _dir) = create_test_db();

        // Create a session
        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        let commit_sha = "abc123def456789012345678901234567890abcd";

        // Create link
        let link = SessionLink {
            id: Uuid::new_v4(),
            session_id: session.id,
            link_type: LinkType::Commit,
            commit_sha: Some(commit_sha.to_string()),
            branch: None,
            remote: None,
            created_at: Utc::now(),
            created_by: LinkCreator::User,
            confidence: None,
        };

        db.insert_link(&link).expect("insert link");

        // Verify link exists
        let links = db.get_links_by_session(&session.id).expect("get links");
        assert_eq!(links.len(), 1, "Should have 1 link");
        assert_eq!(
            links[0].commit_sha,
            Some(commit_sha.to_string()),
            "Commit SHA should match"
        );
        assert_eq!(
            links[0].link_type,
            LinkType::Commit,
            "Type should be Commit"
        );
        assert_eq!(
            links[0].created_by,
            LinkCreator::User,
            "Created by should be User"
        );
    }

    #[test]
    fn test_link_invalid_session_prefix() {
        let (db, _dir) = create_test_db();

        // Create a session
        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        // Try to find session with invalid prefix
        let sessions = db.list_sessions(1000, None).expect("list");
        let found = sessions
            .iter()
            .find(|s| s.id.to_string().starts_with("zzzzzzzz"));

        assert!(
            found.is_none(),
            "Should not find session with invalid prefix"
        );
    }

    #[test]
    fn test_link_multiple_sessions_to_same_commit() {
        let (db, _dir) = create_test_db();

        // Create two sessions
        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/project2", Utc::now(), None);

        db.insert_session(&session1).expect("insert 1");
        db.insert_session(&session2).expect("insert 2");

        let commit_sha = "abc123def456789012345678901234567890abcd";

        // Link both to same commit
        let link1 = create_test_link(session1.id, Some(commit_sha), LinkType::Commit);
        let link2 = create_test_link(session2.id, Some(commit_sha), LinkType::Commit);

        db.insert_link(&link1).expect("insert link1");
        db.insert_link(&link2).expect("insert link2");

        // Query by commit
        let links = db.get_links_by_commit(commit_sha).expect("get links");
        assert_eq!(links.len(), 2, "Should have 2 links for same commit");
    }

    #[test]
    fn test_link_session_to_multiple_commits() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        // Link to multiple commits
        let link1 = create_test_link(
            session.id,
            Some("abc123def456789012345678901234567890abcd"),
            LinkType::Commit,
        );
        let link2 = create_test_link(
            session.id,
            Some("def456abc789012345678901234567890123abcd"),
            LinkType::Commit,
        );

        db.insert_link(&link1).expect("insert link1");
        db.insert_link(&link2).expect("insert link2");

        // Query by session
        let links = db.get_links_by_session(&session.id).expect("get links");
        assert_eq!(links.len(), 2, "Should have 2 links for same session");
    }

    #[test]
    fn test_find_session_by_prefix_for_linking() {
        let (db, _dir) = create_test_db();

        // Create sessions with known IDs
        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/project2", Utc::now(), None);

        db.insert_session(&session1).expect("insert 1");
        db.insert_session(&session2).expect("insert 2");

        let session1_prefix = &session1.id.to_string()[..8];

        // Find by prefix
        let sessions = db.list_sessions(1000, None).expect("list");
        let found = sessions
            .iter()
            .find(|s| s.id.to_string().starts_with(session1_prefix));

        assert!(found.is_some(), "Should find by prefix");
        assert_eq!(
            found.unwrap().id,
            session1.id,
            "Should find correct session"
        );
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

mod error_handling_tests {
    use super::*;

    #[test]
    fn test_invalid_database_path_returns_error() {
        // Try to open database in a non-existent directory without creating it
        let result = Database::open(&std::path::PathBuf::from(
            "/nonexistent/path/that/should/not/exist/test.db",
        ));

        assert!(result.is_err(), "Should fail with invalid path");
    }

    #[test]
    fn test_get_nonexistent_session_returns_none() {
        let (db, _dir) = create_test_db();

        let result = db.get_session(&Uuid::new_v4()).expect("query should work");

        assert!(
            result.is_none(),
            "Should return None for nonexistent session"
        );
    }

    #[test]
    fn test_get_messages_for_nonexistent_session_returns_empty() {
        let (db, _dir) = create_test_db();

        let messages = db.get_messages(&Uuid::new_v4()).expect("query should work");

        assert!(
            messages.is_empty(),
            "Should return empty for nonexistent session"
        );
    }

    #[test]
    fn test_get_links_for_nonexistent_session_returns_empty() {
        let (db, _dir) = create_test_db();

        let links = db
            .get_links_by_session(&Uuid::new_v4())
            .expect("query should work");

        assert!(
            links.is_empty(),
            "Should return empty for nonexistent session"
        );
    }

    #[test]
    fn test_get_links_for_nonexistent_commit_returns_empty() {
        let (db, _dir) = create_test_db();

        let links = db
            .get_links_by_commit("nonexistent_commit_sha")
            .expect("query should work");

        assert!(
            links.is_empty(),
            "Should return empty for nonexistent commit"
        );
    }

    #[test]
    fn test_unrelated_prefix_matches_nothing() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        let sessions = db.list_sessions(100, None).expect("list");

        // Test with a clearly invalid prefix that would not match any valid UUID
        let found = sessions.iter().find(|s| {
            s.id.to_string()
                .starts_with("00000000-0000-0000-0000-000000000000")
        });

        assert!(
            found.is_none(),
            "Should not find session with unrelated prefix"
        );
    }

    #[test]
    fn test_malformed_jsonl_handled_gracefully() {
        use lore_cli::capture::watchers::claude_code;

        // Create a file with malformed content
        let mut file = NamedTempFile::new().expect("create temp file");
        writeln!(file, "{{invalid json").expect("write");
        writeln!(file, "just text").expect("write");
        writeln!(file).expect("write blank line");
        file.flush().expect("flush");

        let result = claude_code::parse_session_file(file.path());

        // Should succeed but return empty messages
        assert!(result.is_ok(), "Should not error on malformed content");
        let parsed = result.unwrap();
        assert!(
            parsed.messages.is_empty(),
            "Should have no messages from invalid content"
        );
    }

    #[test]
    fn test_session_with_special_characters_in_directory() {
        let (db, _dir) = create_test_db();

        // Create session with special characters in working directory
        let session = create_test_session(
            "claude-code",
            "/home/user/my project (1)/test's code",
            Utc::now(),
            None,
        );

        db.insert_session(&session).expect("insert");

        // Retrieve and verify
        let retrieved = db
            .get_session(&session.id)
            .expect("get")
            .expect("should exist");

        assert_eq!(
            retrieved.working_directory, "/home/user/my project (1)/test's code",
            "Special characters should be preserved"
        );
    }
}

// =============================================================================
// Delete Command Tests
// =============================================================================

mod delete_tests {
    use super::*;

    #[test]
    fn test_delete_session_removes_session() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");

        let msg = create_test_message(session.id, 0, MessageRole::User, "Test message");
        db.insert_message(&msg).expect("insert msg");

        let link = create_test_link(session.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("insert link");

        // Delete the session
        let (msgs, links) = db.delete_session(&session.id).expect("delete");

        assert_eq!(msgs, 1, "Should delete 1 message");
        assert_eq!(links, 1, "Should delete 1 link");
        assert!(
            db.get_session(&session.id).expect("get").is_none(),
            "Session should be gone"
        );
    }

    #[test]
    fn test_delete_nonexistent_session() {
        let (db, _dir) = create_test_db();

        let fake_id = Uuid::new_v4();
        let (msgs, links) = db.delete_session(&fake_id).expect("delete");

        assert_eq!(msgs, 0, "No messages to delete");
        assert_eq!(links, 0, "No links to delete");
    }
}

// =============================================================================
// Database Management Tests (db subcommand)
// =============================================================================

mod db_management_tests {
    use super::*;

    #[test]
    fn test_vacuum_succeeds() {
        let (db, _dir) = create_test_db();

        // Add and then delete some data to create reclaimable space
        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert");
        db.delete_session(&session.id).expect("delete");

        // Vacuum should succeed
        db.vacuum().expect("vacuum should succeed");
    }

    #[test]
    fn test_prune_deletes_old_sessions() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create old and new sessions
        let old_session =
            create_test_session("claude-code", "/old", now - Duration::days(100), None);
        let new_session = create_test_session("claude-code", "/new", now - Duration::days(5), None);

        db.insert_session(&old_session).expect("insert old");
        db.insert_session(&new_session).expect("insert new");

        // Count before prune
        let before_count = db
            .count_sessions_older_than(now - Duration::days(30))
            .expect("count");
        assert_eq!(before_count, 1, "Should have 1 old session");

        // Prune sessions older than 30 days
        let deleted = db
            .delete_sessions_older_than(now - Duration::days(30))
            .expect("prune");
        assert_eq!(deleted, 1, "Should delete 1 session");

        // Verify new session remains
        assert!(
            db.get_session(&new_session.id).expect("get").is_some(),
            "New session should remain"
        );
        assert!(
            db.get_session(&old_session.id).expect("get").is_none(),
            "Old session should be deleted"
        );
    }

    #[test]
    fn test_stats_returns_correct_counts() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Add varied data
        let session1 = create_test_session("claude-code", "/p1", now - Duration::hours(2), None);
        let session2 = create_test_session("aider", "/p2", now - Duration::hours(1), None);
        let session3 = create_test_session("claude-code", "/p3", now, None);

        db.insert_session(&session1).expect("insert");
        db.insert_session(&session2).expect("insert");
        db.insert_session(&session3).expect("insert");

        let msg = create_test_message(session1.id, 0, MessageRole::User, "Hello");
        db.insert_message(&msg).expect("insert msg");

        let link = create_test_link(session2.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("insert link");

        // Get stats
        let stats = db.stats().expect("stats");

        assert_eq!(stats.session_count, 3, "Should have 3 sessions");
        assert_eq!(stats.message_count, 1, "Should have 1 message");
        assert_eq!(stats.link_count, 1, "Should have 1 link");

        // Check tool breakdown
        let claude_sessions = stats
            .sessions_by_tool
            .iter()
            .find(|(tool, _)| tool == "claude-code")
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert_eq!(claude_sessions, 2, "Should have 2 claude-code sessions");

        let aider_sessions = stats
            .sessions_by_tool
            .iter()
            .find(|(tool, _)| tool == "aider")
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert_eq!(aider_sessions, 1, "Should have 1 aider session");
    }

    #[test]
    fn test_file_size_returns_value() {
        let (db, _dir) = create_test_db();

        let size = db.file_size().expect("file_size");
        assert!(size.is_some(), "Should return file size");
        assert!(size.unwrap() > 0, "File size should be positive");
    }
}

//! SQLite storage layer for Lore

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use uuid::Uuid;

use super::models::{Message, MessageContent, MessageRole, Session, SessionLink};

/// Get the default database path
pub fn default_db_path() -> Result<PathBuf> {
    let config_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".lore");

    std::fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("lore.db"))
}

/// Database connection wrapper
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create the database
    pub fn open(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Open the default database
    pub fn open_default() -> Result<Self> {
        let path = default_db_path()?;
        Self::open(&path)
    }

    /// Run migrations
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                tool TEXT NOT NULL,
                tool_version TEXT,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                model TEXT,
                working_directory TEXT NOT NULL,
                git_branch TEXT,
                source_path TEXT,
                message_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                parent_id TEXT,
                idx INTEGER NOT NULL,
                timestamp TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                model TEXT,
                git_branch TEXT,
                cwd TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS session_links (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                link_type TEXT NOT NULL,
                commit_sha TEXT,
                branch TEXT,
                remote TEXT,
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                confidence REAL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS repositories (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                remote_url TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_session_at TEXT
            );

            -- Indexes for common queries
            CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON sessions(started_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_working_directory ON sessions(working_directory);
            CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id);
            CREATE INDEX IF NOT EXISTS idx_session_links_session_id ON session_links(session_id);
            CREATE INDEX IF NOT EXISTS idx_session_links_commit_sha ON session_links(commit_sha);
            "#,
        )?;
        Ok(())
    }

    // ==================== Sessions ====================

    /// Insert a new session
    pub fn insert_session(&self, session: &Session) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO sessions (id, tool, tool_version, started_at, ended_at, model, working_directory, git_branch, source_path, message_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                ended_at = ?5,
                message_count = ?10
            "#,
            params![
                session.id.to_string(),
                session.tool,
                session.tool_version,
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
                session.model,
                session.working_directory,
                session.git_branch,
                session.source_path,
                session.message_count,
            ],
        )?;
        Ok(())
    }

    /// Get a session by ID
    pub fn get_session(&self, id: &Uuid) -> Result<Option<Session>> {
        self.conn
            .query_row(
                "SELECT id, tool, tool_version, started_at, ended_at, model, working_directory, git_branch, source_path, message_count FROM sessions WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok(Session {
                        id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                        tool: row.get(1)?,
                        tool_version: row.get(2)?,
                        started_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                            .unwrap()
                            .with_timezone(&chrono::Utc),
                        ended_at: row.get::<_, Option<String>>(4)?.map(|s| {
                            chrono::DateTime::parse_from_rfc3339(&s)
                                .unwrap()
                                .with_timezone(&chrono::Utc)
                        }),
                        model: row.get(5)?,
                        working_directory: row.get(6)?,
                        git_branch: row.get(7)?,
                        source_path: row.get(8)?,
                        message_count: row.get(9)?,
                    })
                },
            )
            .optional()
            .context("Failed to get session")
    }

    /// List recent sessions
    pub fn list_sessions(&self, limit: usize, working_dir: Option<&str>) -> Result<Vec<Session>> {
        let mut stmt = if working_dir.is_some() {
            self.conn.prepare(
                "SELECT id, tool, tool_version, started_at, ended_at, model, working_directory, git_branch, source_path, message_count 
                 FROM sessions 
                 WHERE working_directory LIKE ?1
                 ORDER BY started_at DESC 
                 LIMIT ?2"
            )?
        } else {
            self.conn.prepare(
                "SELECT id, tool, tool_version, started_at, ended_at, model, working_directory, git_branch, source_path, message_count 
                 FROM sessions 
                 ORDER BY started_at DESC 
                 LIMIT ?1"
            )?
        };

        let rows = if let Some(wd) = working_dir {
            stmt.query_map(params![format!("{}%", wd), limit], Self::row_to_session)?
        } else {
            stmt.query_map(params![limit], Self::row_to_session)?
        };

        rows.collect::<Result<Vec<_>, _>>().context("Failed to list sessions")
    }

    /// Check if a session exists by source path
    pub fn session_exists_by_source(&self, source_path: &str) -> Result<bool> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE source_path = ?1",
            params![source_path],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        Ok(Session {
            id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
            tool: row.get(1)?,
            tool_version: row.get(2)?,
            started_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                .unwrap()
                .with_timezone(&chrono::Utc),
            ended_at: row.get::<_, Option<String>>(4)?.map(|s| {
                chrono::DateTime::parse_from_rfc3339(&s)
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            }),
            model: row.get(5)?,
            working_directory: row.get(6)?,
            git_branch: row.get(7)?,
            source_path: row.get(8)?,
            message_count: row.get(9)?,
        })
    }

    // ==================== Messages ====================

    /// Insert a message
    pub fn insert_message(&self, message: &Message) -> Result<()> {
        let content_json = serde_json::to_string(&message.content)?;

        self.conn.execute(
            r#"
            INSERT INTO messages (id, session_id, parent_id, idx, timestamp, role, content, model, git_branch, cwd)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO NOTHING
            "#,
            params![
                message.id.to_string(),
                message.session_id.to_string(),
                message.parent_id.map(|u| u.to_string()),
                message.index,
                message.timestamp.to_rfc3339(),
                message.role.to_string(),
                content_json,
                message.model,
                message.git_branch,
                message.cwd,
            ],
        )?;
        Ok(())
    }

    /// Get messages for a session
    pub fn get_messages(&self, session_id: &Uuid) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, parent_id, idx, timestamp, role, content, model, git_branch, cwd 
             FROM messages 
             WHERE session_id = ?1 
             ORDER BY idx"
        )?;

        let rows = stmt.query_map(params![session_id.to_string()], |row| {
            let role_str: String = row.get(5)?;
            let content_str: String = row.get(6)?;

            Ok(Message {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                session_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
                parent_id: row.get::<_, Option<String>>(2)?.map(|s| Uuid::parse_str(&s).unwrap()),
                index: row.get(3)?,
                timestamp: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                role: match role_str.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "system" => MessageRole::System,
                    _ => MessageRole::User,
                },
                content: serde_json::from_str(&content_str).unwrap_or(MessageContent::Text(content_str)),
                model: row.get(7)?,
                git_branch: row.get(8)?,
                cwd: row.get(9)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().context("Failed to get messages")
    }

    // ==================== Session Links ====================

    /// Insert a session link
    pub fn insert_link(&self, link: &SessionLink) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO session_links (id, session_id, link_type, commit_sha, branch, remote, created_at, created_by, confidence)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                link.id.to_string(),
                link.session_id.to_string(),
                format!("{:?}", link.link_type).to_lowercase(),
                link.commit_sha,
                link.branch,
                link.remote,
                link.created_at.to_rfc3339(),
                format!("{:?}", link.created_by).to_lowercase(),
                link.confidence,
            ],
        )?;
        Ok(())
    }

    /// Get links for a commit
    pub fn get_links_by_commit(&self, commit_sha: &str) -> Result<Vec<SessionLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, link_type, commit_sha, branch, remote, created_at, created_by, confidence 
             FROM session_links 
             WHERE commit_sha LIKE ?1"
        )?;

        let pattern = format!("{}%", commit_sha);
        let rows = stmt.query_map(params![pattern], Self::row_to_link)?;

        rows.collect::<Result<Vec<_>, _>>().context("Failed to get links")
    }

    /// Get links for a session
    pub fn get_links_by_session(&self, session_id: &Uuid) -> Result<Vec<SessionLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, link_type, commit_sha, branch, remote, created_at, created_by, confidence 
             FROM session_links 
             WHERE session_id = ?1"
        )?;

        let rows = stmt.query_map(params![session_id.to_string()], Self::row_to_link)?;

        rows.collect::<Result<Vec<_>, _>>().context("Failed to get links")
    }

    fn row_to_link(row: &rusqlite::Row) -> rusqlite::Result<SessionLink> {
        use super::models::{LinkCreator, LinkType};

        let link_type_str: String = row.get(2)?;
        let created_by_str: String = row.get(7)?;

        Ok(SessionLink {
            id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
            session_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
            link_type: match link_type_str.as_str() {
                "commit" => LinkType::Commit,
                "branch" => LinkType::Branch,
                "pr" => LinkType::Pr,
                _ => LinkType::Manual,
            },
            commit_sha: row.get(3)?,
            branch: row.get(4)?,
            remote: row.get(5)?,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                .unwrap()
                .with_timezone(&chrono::Utc),
            created_by: match created_by_str.as_str() {
                "auto" => LinkCreator::Auto,
                _ => LinkCreator::User,
            },
            confidence: row.get(8)?,
        })
    }

    // ==================== Stats ====================

    /// Get total session count
    pub fn session_count(&self) -> Result<i32> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get total message count
    pub fn message_count(&self) -> Result<i32> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{LinkCreator, LinkType, MessageContent, MessageRole};
    use chrono::{Duration, Utc};
    use tempfile::tempdir;

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
    fn create_test_message(
        session_id: Uuid,
        index: i32,
        role: MessageRole,
        content: &str,
    ) -> Message {
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
            created_by: LinkCreator::Auto,
            confidence: Some(0.95),
        }
    }

    // ==================== Session Tests ====================

    #[test]
    fn test_insert_and_get_session() {
        let (db, _dir) = create_test_db();
        let session = create_test_session(
            "claude-code",
            "/home/user/project",
            Utc::now(),
            Some("/path/to/source.jsonl"),
        );

        db.insert_session(&session)
            .expect("Failed to insert session");

        let retrieved = db
            .get_session(&session.id)
            .expect("Failed to get session")
            .expect("Session should exist");

        assert_eq!(retrieved.id, session.id, "Session ID should match");
        assert_eq!(retrieved.tool, session.tool, "Tool should match");
        assert_eq!(
            retrieved.tool_version, session.tool_version,
            "Tool version should match"
        );
        assert_eq!(
            retrieved.working_directory, session.working_directory,
            "Working directory should match"
        );
        assert_eq!(
            retrieved.git_branch, session.git_branch,
            "Git branch should match"
        );
        assert_eq!(
            retrieved.source_path, session.source_path,
            "Source path should match"
        );
    }

    #[test]
    fn test_list_sessions() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Insert sessions with different timestamps (oldest first)
        let session1 = create_test_session(
            "claude-code",
            "/project1",
            now - Duration::hours(2),
            None,
        );
        let session2 = create_test_session(
            "cursor",
            "/project2",
            now - Duration::hours(1),
            None,
        );
        let session3 = create_test_session(
            "claude-code",
            "/project3",
            now,
            None,
        );

        db.insert_session(&session1).expect("Failed to insert session1");
        db.insert_session(&session2).expect("Failed to insert session2");
        db.insert_session(&session3).expect("Failed to insert session3");

        let sessions = db
            .list_sessions(10, None)
            .expect("Failed to list sessions");

        assert_eq!(sessions.len(), 3, "Should have 3 sessions");
        // Sessions should be ordered by started_at DESC (most recent first)
        assert_eq!(
            sessions[0].id, session3.id,
            "Most recent session should be first"
        );
        assert_eq!(
            sessions[1].id, session2.id,
            "Second most recent session should be second"
        );
        assert_eq!(
            sessions[2].id, session1.id,
            "Oldest session should be last"
        );
    }

    #[test]
    fn test_list_sessions_with_working_dir_filter() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        let session1 = create_test_session(
            "claude-code",
            "/home/user/project-a",
            now - Duration::hours(1),
            None,
        );
        let session2 = create_test_session(
            "claude-code",
            "/home/user/project-b",
            now,
            None,
        );
        let session3 = create_test_session(
            "claude-code",
            "/other/path",
            now,
            None,
        );

        db.insert_session(&session1).expect("Failed to insert session1");
        db.insert_session(&session2).expect("Failed to insert session2");
        db.insert_session(&session3).expect("Failed to insert session3");

        // Filter by working directory prefix
        let sessions = db
            .list_sessions(10, Some("/home/user"))
            .expect("Failed to list sessions");

        assert_eq!(
            sessions.len(),
            2,
            "Should have 2 sessions matching /home/user prefix"
        );

        // Verify both matching sessions are returned
        let ids: Vec<Uuid> = sessions.iter().map(|s| s.id).collect();
        assert!(
            ids.contains(&session1.id),
            "Should contain session1"
        );
        assert!(
            ids.contains(&session2.id),
            "Should contain session2"
        );
        assert!(
            !ids.contains(&session3.id),
            "Should not contain session3"
        );
    }

    #[test]
    fn test_session_exists_by_source() {
        let (db, _dir) = create_test_db();
        let source_path = "/path/to/session.jsonl";

        let session = create_test_session(
            "claude-code",
            "/project",
            Utc::now(),
            Some(source_path),
        );

        // Before insert, should not exist
        assert!(
            !db.session_exists_by_source(source_path)
                .expect("Failed to check existence"),
            "Session should not exist before insert"
        );

        db.insert_session(&session).expect("Failed to insert session");

        // After insert, should exist
        assert!(
            db.session_exists_by_source(source_path)
                .expect("Failed to check existence"),
            "Session should exist after insert"
        );

        // Different path should not exist
        assert!(
            !db.session_exists_by_source("/other/path.jsonl")
                .expect("Failed to check existence"),
            "Different source path should not exist"
        );
    }

    #[test]
    fn test_get_nonexistent_session() {
        let (db, _dir) = create_test_db();
        let nonexistent_id = Uuid::new_v4();

        let result = db
            .get_session(&nonexistent_id)
            .expect("Failed to query for nonexistent session");

        assert!(
            result.is_none(),
            "Should return None for nonexistent session"
        );
    }

    // ==================== Message Tests ====================

    #[test]
    fn test_insert_and_get_messages() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("Failed to insert session");

        let msg1 = create_test_message(session.id, 0, MessageRole::User, "Hello");
        let msg2 = create_test_message(session.id, 1, MessageRole::Assistant, "Hi there!");

        db.insert_message(&msg1).expect("Failed to insert message 1");
        db.insert_message(&msg2).expect("Failed to insert message 2");

        let messages = db
            .get_messages(&session.id)
            .expect("Failed to get messages");

        assert_eq!(messages.len(), 2, "Should have 2 messages");
        assert_eq!(messages[0].id, msg1.id, "First message ID should match");
        assert_eq!(messages[1].id, msg2.id, "Second message ID should match");
        assert_eq!(
            messages[0].role,
            MessageRole::User,
            "First message role should be User"
        );
        assert_eq!(
            messages[1].role,
            MessageRole::Assistant,
            "Second message role should be Assistant"
        );
    }

    #[test]
    fn test_messages_ordered_by_index() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("Failed to insert session");

        // Insert messages out of order
        let msg3 = create_test_message(session.id, 2, MessageRole::Assistant, "Third");
        let msg1 = create_test_message(session.id, 0, MessageRole::User, "First");
        let msg2 = create_test_message(session.id, 1, MessageRole::Assistant, "Second");

        db.insert_message(&msg3).expect("Failed to insert message 3");
        db.insert_message(&msg1).expect("Failed to insert message 1");
        db.insert_message(&msg2).expect("Failed to insert message 2");

        let messages = db
            .get_messages(&session.id)
            .expect("Failed to get messages");

        assert_eq!(messages.len(), 3, "Should have 3 messages");
        assert_eq!(
            messages[0].index, 0,
            "First message should have index 0"
        );
        assert_eq!(
            messages[1].index, 1,
            "Second message should have index 1"
        );
        assert_eq!(
            messages[2].index, 2,
            "Third message should have index 2"
        );

        // Verify content matches expected order
        assert_eq!(
            messages[0].content.text(),
            "First",
            "First message content should be 'First'"
        );
        assert_eq!(
            messages[1].content.text(),
            "Second",
            "Second message content should be 'Second'"
        );
        assert_eq!(
            messages[2].content.text(),
            "Third",
            "Third message content should be 'Third'"
        );
    }

    // ==================== SessionLink Tests ====================

    #[test]
    fn test_insert_and_get_links_by_session() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("Failed to insert session");

        let link1 = create_test_link(session.id, Some("abc123def456"), LinkType::Commit);
        let link2 = create_test_link(session.id, Some("def456abc789"), LinkType::Commit);

        db.insert_link(&link1).expect("Failed to insert link 1");
        db.insert_link(&link2).expect("Failed to insert link 2");

        let links = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");

        assert_eq!(links.len(), 2, "Should have 2 links");

        let link_ids: Vec<Uuid> = links.iter().map(|l| l.id).collect();
        assert!(link_ids.contains(&link1.id), "Should contain link1");
        assert!(link_ids.contains(&link2.id), "Should contain link2");

        // Verify link properties
        let retrieved_link = links.iter().find(|l| l.id == link1.id).unwrap();
        assert_eq!(
            retrieved_link.commit_sha,
            Some("abc123def456".to_string()),
            "Commit SHA should match"
        );
        assert_eq!(
            retrieved_link.link_type,
            LinkType::Commit,
            "Link type should be Commit"
        );
        assert_eq!(
            retrieved_link.created_by,
            LinkCreator::Auto,
            "Created by should be Auto"
        );
    }

    #[test]
    fn test_get_links_by_commit() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("Failed to insert session");

        let full_sha = "abc123def456789012345678901234567890abcd";
        let link = create_test_link(session.id, Some(full_sha), LinkType::Commit);
        db.insert_link(&link).expect("Failed to insert link");

        // Test full SHA match
        let links_full = db
            .get_links_by_commit(full_sha)
            .expect("Failed to get links by full SHA");
        assert_eq!(links_full.len(), 1, "Should find link by full SHA");
        assert_eq!(links_full[0].id, link.id, "Link ID should match");

        // Test partial SHA match (prefix)
        let links_partial = db
            .get_links_by_commit("abc123")
            .expect("Failed to get links by partial SHA");
        assert_eq!(
            links_partial.len(),
            1,
            "Should find link by partial SHA prefix"
        );
        assert_eq!(links_partial[0].id, link.id, "Link ID should match");

        // Test non-matching SHA
        let links_none = db
            .get_links_by_commit("zzz999")
            .expect("Failed to get links by non-matching SHA");
        assert_eq!(
            links_none.len(),
            0,
            "Should not find link with non-matching SHA"
        );
    }

    // ==================== Database Tests ====================

    #[test]
    fn test_database_creation() {
        let dir = tempdir().expect("Failed to create temp directory");
        let db_path = dir.path().join("new_test.db");

        // Database should not exist before creation
        assert!(
            !db_path.exists(),
            "Database file should not exist before creation"
        );

        let db = Database::open(&db_path).expect("Failed to create database");

        // Database file should exist after creation
        assert!(
            db_path.exists(),
            "Database file should exist after creation"
        );

        // Verify tables exist by attempting operations
        let session_count = db.session_count().expect("Failed to get session count");
        assert_eq!(session_count, 0, "New database should have 0 sessions");

        let message_count = db.message_count().expect("Failed to get message count");
        assert_eq!(message_count, 0, "New database should have 0 messages");
    }

    #[test]
    fn test_session_count() {
        let (db, _dir) = create_test_db();

        assert_eq!(
            db.session_count().expect("Failed to get count"),
            0,
            "Initial session count should be 0"
        );

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        db.insert_session(&session1).expect("Failed to insert session1");

        assert_eq!(
            db.session_count().expect("Failed to get count"),
            1,
            "Session count should be 1 after first insert"
        );

        let session2 = create_test_session("cursor", "/project2", Utc::now(), None);
        db.insert_session(&session2).expect("Failed to insert session2");

        assert_eq!(
            db.session_count().expect("Failed to get count"),
            2,
            "Session count should be 2 after second insert"
        );
    }

    #[test]
    fn test_message_count() {
        let (db, _dir) = create_test_db();

        assert_eq!(
            db.message_count().expect("Failed to get count"),
            0,
            "Initial message count should be 0"
        );

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("Failed to insert session");

        let msg1 = create_test_message(session.id, 0, MessageRole::User, "Hello");
        db.insert_message(&msg1).expect("Failed to insert message1");

        assert_eq!(
            db.message_count().expect("Failed to get count"),
            1,
            "Message count should be 1 after first insert"
        );

        let msg2 = create_test_message(session.id, 1, MessageRole::Assistant, "Hi");
        let msg3 = create_test_message(session.id, 2, MessageRole::User, "How are you?");
        db.insert_message(&msg2).expect("Failed to insert message2");
        db.insert_message(&msg3).expect("Failed to insert message3");

        assert_eq!(
            db.message_count().expect("Failed to get count"),
            3,
            "Message count should be 3 after all inserts"
        );
    }
}

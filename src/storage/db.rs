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

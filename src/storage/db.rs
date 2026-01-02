//! SQLite storage layer for Lore.
//!
//! Provides database operations for storing and retrieving sessions,
//! messages, and session-to-commit links. Uses SQLite for local-first
//! persistence with automatic schema migrations.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use uuid::Uuid;

use super::models::{Message, MessageContent, MessageRole, SearchResult, Session, SessionLink};

/// Parses a UUID from a string, converting errors to rusqlite errors.
///
/// Used in row mapping functions where we need to return rusqlite::Result.
fn parse_uuid(s: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Parses an RFC3339 datetime string, converting errors to rusqlite errors.
///
/// Used in row mapping functions where we need to return rusqlite::Result.
fn parse_datetime(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

/// Escapes a query string for FTS5 by wrapping each word in double quotes.
///
/// FTS5 has special syntax characters (e.g., /, *, AND, OR, NOT) that need
/// escaping to be treated as literal search terms.
fn escape_fts5_query(query: &str) -> String {
    // Split on whitespace and wrap each word in quotes, escaping internal quotes
    query
        .split_whitespace()
        .map(|word| {
            let escaped = word.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns the default database path at `~/.lore/lore.db`.
///
/// Creates the `.lore` directory if it does not exist.
pub fn default_db_path() -> Result<PathBuf> {
    let config_dir = dirs::home_dir()
        .context("Could not find home directory. Ensure your HOME environment variable is set.")?
        .join(".lore");

    std::fs::create_dir_all(&config_dir).with_context(|| {
        format!(
            "Failed to create Lore data directory at {}. Check directory permissions.",
            config_dir.display()
        )
    })?;
    Ok(config_dir.join("lore.db"))
}

/// SQLite database connection wrapper.
///
/// Provides methods for storing and querying sessions, messages,
/// and session-to-commit links. Handles schema migrations automatically
/// when opening the database.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Opens or creates a database at the specified path.
    ///
    /// Runs schema migrations automatically to ensure tables exist.
    pub fn open(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Opens the default database at `~/.lore/lore.db`.
    ///
    /// Creates the database file and directory if they do not exist.
    pub fn open_default() -> Result<Self> {
        let path = default_db_path()?;
        Self::open(&path)
    }

    /// Runs database schema migrations.
    ///
    /// Creates tables for sessions, messages, session_links, and repositories
    /// if they do not already exist. Also creates indexes for common queries.
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

        // Create FTS5 virtual table for full-text search on message content.
        // This is a standalone FTS table (not content-synced) because we need to
        // store extracted text content, not the raw JSON from the messages table.
        // The message_id column stores the UUID string for joining back to messages.
        self.conn.execute_batch(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                message_id,
                text_content,
                tokenize='porter unicode61'
            );
            "#,
        )?;

        // Create FTS5 virtual table for session metadata search.
        // Allows searching by project name, branch, tool, and working directory.
        self.conn.execute_batch(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
                session_id,
                tool,
                working_directory,
                git_branch,
                tokenize='porter unicode61'
            );
            "#,
        )?;

        Ok(())
    }

    // ==================== Sessions ====================

    /// Inserts a new session or updates an existing one.
    ///
    /// If a session with the same ID already exists, updates the `ended_at`
    /// and `message_count` fields. Also updates the sessions_fts index for
    /// full-text search on session metadata.
    pub fn insert_session(&self, session: &Session) -> Result<()> {
        let rows_changed = self.conn.execute(
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

        // Insert into sessions_fts for metadata search (only on new inserts)
        if rows_changed > 0 {
            // Check if already in FTS (for ON CONFLICT case)
            let fts_count: i32 = self.conn.query_row(
                "SELECT COUNT(*) FROM sessions_fts WHERE session_id = ?1",
                params![session.id.to_string()],
                |row| row.get(0),
            )?;

            if fts_count == 0 {
                self.conn.execute(
                    "INSERT INTO sessions_fts (session_id, tool, working_directory, git_branch) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        session.id.to_string(),
                        session.tool,
                        session.working_directory,
                        session.git_branch.as_deref().unwrap_or(""),
                    ],
                )?;
            }
        }

        Ok(())
    }

    /// Retrieves a session by its unique ID.
    ///
    /// Returns `None` if no session with the given ID exists.
    pub fn get_session(&self, id: &Uuid) -> Result<Option<Session>> {
        self.conn
            .query_row(
                "SELECT id, tool, tool_version, started_at, ended_at, model, working_directory, git_branch, source_path, message_count FROM sessions WHERE id = ?1",
                params![id.to_string()],
                Self::row_to_session,
            )
            .optional()
            .context("Failed to get session")
    }

    /// Lists sessions ordered by start time (most recent first).
    ///
    /// Optionally filters by working directory prefix. Returns at most
    /// `limit` sessions.
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

        rows.collect::<Result<Vec<_>, _>>()
            .context("Failed to list sessions")
    }

    /// Checks if a session with the given source path already exists.
    ///
    /// Used to detect already-imported sessions during import operations.
    pub fn session_exists_by_source(&self, source_path: &str) -> Result<bool> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE source_path = ?1",
            params![source_path],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        let ended_at_str: Option<String> = row.get(4)?;
        let ended_at = match ended_at_str {
            Some(s) => Some(parse_datetime(&s)?),
            None => None,
        };

        Ok(Session {
            id: parse_uuid(&row.get::<_, String>(0)?)?,
            tool: row.get(1)?,
            tool_version: row.get(2)?,
            started_at: parse_datetime(&row.get::<_, String>(3)?)?,
            ended_at,
            model: row.get(5)?,
            working_directory: row.get(6)?,
            git_branch: row.get(7)?,
            source_path: row.get(8)?,
            message_count: row.get(9)?,
        })
    }

    // ==================== Messages ====================

    /// Inserts a message into the database.
    ///
    /// If a message with the same ID already exists, the insert is ignored.
    /// Message content is serialized to JSON for storage. Also inserts
    /// extracted text content into the FTS index for full-text search.
    pub fn insert_message(&self, message: &Message) -> Result<()> {
        let content_json = serde_json::to_string(&message.content)?;

        let rows_changed = self.conn.execute(
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

        // Only insert into FTS if the message was actually inserted (not a duplicate)
        if rows_changed > 0 {
            let text_content = message.content.text();
            if !text_content.is_empty() {
                self.conn.execute(
                    "INSERT INTO messages_fts (message_id, text_content) VALUES (?1, ?2)",
                    params![message.id.to_string(), text_content],
                )?;
            }
        }

        Ok(())
    }

    /// Retrieves all messages for a session, ordered by index.
    ///
    /// Messages are returned in conversation order (by their `index` field).
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

            let parent_id_str: Option<String> = row.get(2)?;
            let parent_id = match parent_id_str {
                Some(s) => Some(parse_uuid(&s)?),
                None => None,
            };

            Ok(Message {
                id: parse_uuid(&row.get::<_, String>(0)?)?,
                session_id: parse_uuid(&row.get::<_, String>(1)?)?,
                parent_id,
                index: row.get(3)?,
                timestamp: parse_datetime(&row.get::<_, String>(4)?)?,
                role: match role_str.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "system" => MessageRole::System,
                    _ => MessageRole::User,
                },
                content: serde_json::from_str(&content_str)
                    .unwrap_or(MessageContent::Text(content_str)),
                model: row.get(7)?,
                git_branch: row.get(8)?,
                cwd: row.get(9)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .context("Failed to get messages")
    }

    // ==================== Session Links ====================

    /// Inserts a link between a session and a git commit.
    ///
    /// Links can be created manually by users or automatically by
    /// the auto-linking system based on time and file overlap heuristics.
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

    /// Retrieves all session links for a commit.
    ///
    /// Supports prefix matching on the commit SHA, allowing short SHAs
    /// (e.g., first 8 characters) to be used for lookup.
    pub fn get_links_by_commit(&self, commit_sha: &str) -> Result<Vec<SessionLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, link_type, commit_sha, branch, remote, created_at, created_by, confidence 
             FROM session_links 
             WHERE commit_sha LIKE ?1"
        )?;

        let pattern = format!("{commit_sha}%");
        let rows = stmt.query_map(params![pattern], Self::row_to_link)?;

        rows.collect::<Result<Vec<_>, _>>()
            .context("Failed to get links")
    }

    /// Retrieves all links associated with a session.
    ///
    /// A session can be linked to multiple commits if it spans
    /// several git operations.
    pub fn get_links_by_session(&self, session_id: &Uuid) -> Result<Vec<SessionLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, link_type, commit_sha, branch, remote, created_at, created_by, confidence 
             FROM session_links 
             WHERE session_id = ?1"
        )?;

        let rows = stmt.query_map(params![session_id.to_string()], Self::row_to_link)?;

        rows.collect::<Result<Vec<_>, _>>()
            .context("Failed to get links")
    }

    fn row_to_link(row: &rusqlite::Row) -> rusqlite::Result<SessionLink> {
        use super::models::{LinkCreator, LinkType};

        let link_type_str: String = row.get(2)?;
        let created_by_str: String = row.get(7)?;

        Ok(SessionLink {
            id: parse_uuid(&row.get::<_, String>(0)?)?,
            session_id: parse_uuid(&row.get::<_, String>(1)?)?,
            link_type: match link_type_str.as_str() {
                "commit" => LinkType::Commit,
                "branch" => LinkType::Branch,
                "pr" => LinkType::Pr,
                _ => LinkType::Manual,
            },
            commit_sha: row.get(3)?,
            branch: row.get(4)?,
            remote: row.get(5)?,
            created_at: parse_datetime(&row.get::<_, String>(6)?)?,
            created_by: match created_by_str.as_str() {
                "auto" => LinkCreator::Auto,
                _ => LinkCreator::User,
            },
            confidence: row.get(8)?,
        })
    }

    /// Deletes a specific session link by its ID.
    ///
    /// Returns `true` if a link was deleted, `false` if no link with that ID existed.
    ///
    /// Note: This method is part of the public API for programmatic use,
    /// though the CLI currently uses session/commit-based deletion.
    #[allow(dead_code)]
    pub fn delete_link(&self, link_id: &Uuid) -> Result<bool> {
        let rows_affected = self.conn.execute(
            "DELETE FROM session_links WHERE id = ?1",
            params![link_id.to_string()],
        )?;
        Ok(rows_affected > 0)
    }

    /// Deletes all links for a session.
    ///
    /// Returns the number of links deleted.
    pub fn delete_links_by_session(&self, session_id: &Uuid) -> Result<usize> {
        let rows_affected = self.conn.execute(
            "DELETE FROM session_links WHERE session_id = ?1",
            params![session_id.to_string()],
        )?;
        Ok(rows_affected)
    }

    /// Deletes a link between a specific session and commit.
    ///
    /// The commit_sha is matched as a prefix, so short SHAs work.
    /// Returns `true` if a link was deleted, `false` if no matching link existed.
    pub fn delete_link_by_session_and_commit(
        &self,
        session_id: &Uuid,
        commit_sha: &str,
    ) -> Result<bool> {
        let pattern = format!("{commit_sha}%");
        let rows_affected = self.conn.execute(
            "DELETE FROM session_links WHERE session_id = ?1 AND commit_sha LIKE ?2",
            params![session_id.to_string(), pattern],
        )?;
        Ok(rows_affected > 0)
    }

    // ==================== Search ====================

    /// Searches message content using full-text search.
    ///
    /// Uses SQLite FTS5 to search for messages matching the query.
    /// Returns results ordered by FTS5 relevance ranking.
    ///
    /// Optional filters:
    /// - `working_dir`: Filter by working directory prefix
    /// - `since`: Filter by minimum timestamp
    /// - `role`: Filter by message role
    ///
    /// Note: This is the legacy search API. For new code, use `search_with_options`.
    #[allow(dead_code)]
    pub fn search_messages(
        &self,
        query: &str,
        limit: usize,
        working_dir: Option<&str>,
        since: Option<chrono::DateTime<chrono::Utc>>,
        role: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        use super::models::SearchOptions;

        // Convert to SearchOptions and use the new method
        let options = SearchOptions {
            query: query.to_string(),
            limit,
            repo: working_dir.map(|s| s.to_string()),
            since,
            role: role.map(|s| s.to_string()),
            ..Default::default()
        };

        self.search_with_options(&options)
    }

    /// Searches messages and session metadata using full-text search with filters.
    ///
    /// Uses SQLite FTS5 to search for messages matching the query.
    /// Also searches session metadata (tool, project, branch) via sessions_fts.
    /// Returns results ordered by FTS5 relevance ranking.
    ///
    /// Supports extensive filtering via SearchOptions:
    /// - `tool`: Filter by AI tool name
    /// - `since`/`until`: Filter by date range
    /// - `project`: Filter by project name (partial match)
    /// - `branch`: Filter by git branch (partial match)
    /// - `role`: Filter by message role
    /// - `repo`: Filter by working directory prefix
    pub fn search_with_options(
        &self,
        options: &super::models::SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        // Escape the query for FTS5 to handle special characters
        let escaped_query = escape_fts5_query(&options.query);

        // Build the query dynamically based on filters
        // Use UNION to search both message content and session metadata
        let mut sql = String::from(
            r#"
            SELECT
                m.session_id,
                m.id as message_id,
                m.role,
                snippet(messages_fts, 1, '**', '**', '...', 32) as snippet,
                m.timestamp,
                s.working_directory,
                s.tool,
                s.git_branch,
                s.message_count,
                s.started_at,
                m.idx as message_index
            FROM messages_fts fts
            JOIN messages m ON fts.message_id = m.id
            JOIN sessions s ON m.session_id = s.id
            WHERE messages_fts MATCH ?1
            "#,
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(escaped_query.clone())];
        let mut param_idx = 2;

        // Add filters
        if options.repo.is_some() {
            sql.push_str(&format!(" AND s.working_directory LIKE ?{param_idx}"));
            param_idx += 1;
        }
        if options.tool.is_some() {
            sql.push_str(&format!(" AND LOWER(s.tool) = LOWER(?{param_idx})"));
            param_idx += 1;
        }
        if options.since.is_some() {
            sql.push_str(&format!(" AND s.started_at >= ?{param_idx}"));
            param_idx += 1;
        }
        if options.until.is_some() {
            sql.push_str(&format!(" AND s.started_at <= ?{param_idx}"));
            param_idx += 1;
        }
        if options.project.is_some() {
            sql.push_str(&format!(" AND s.working_directory LIKE ?{param_idx}"));
            param_idx += 1;
        }
        if options.branch.is_some() {
            sql.push_str(&format!(" AND s.git_branch LIKE ?{param_idx}"));
            param_idx += 1;
        }
        if options.role.is_some() {
            sql.push_str(&format!(" AND m.role = ?{param_idx}"));
            param_idx += 1;
        }

        // Build first SELECT parameter list (after the FTS query param which is already in params_vec)
        if let Some(ref wd) = options.repo {
            params_vec.push(Box::new(format!("{wd}%")));
        }
        if let Some(ref tool) = options.tool {
            params_vec.push(Box::new(tool.clone()));
        }
        if let Some(ts) = options.since {
            params_vec.push(Box::new(ts.to_rfc3339()));
        }
        if let Some(ts) = options.until {
            params_vec.push(Box::new(ts.to_rfc3339()));
        }
        if let Some(ref project) = options.project {
            params_vec.push(Box::new(format!("%{project}%")));
        }
        if let Some(ref branch) = options.branch {
            params_vec.push(Box::new(format!("%{branch}%")));
        }
        if let Some(ref role) = options.role {
            params_vec.push(Box::new(role.clone()));
        }

        // Add UNION for session metadata search (only if not filtering by role)
        // This finds sessions where the metadata matches, returning the first message as representative
        // Uses LIKE patterns instead of FTS5 for metadata since paths contain special characters
        let include_metadata_search = options.role.is_none();
        let metadata_query_pattern = format!("%{}%", options.query);

        if include_metadata_search {
            // For the metadata search, we need 3 separate params for the OR conditions
            let meta_param1 = param_idx;
            let meta_param2 = param_idx + 1;
            let meta_param3 = param_idx + 2;
            param_idx += 3;

            sql.push_str(&format!(
                r#"
            UNION
            SELECT
                s.id as session_id,
                (SELECT id FROM messages WHERE session_id = s.id ORDER BY idx LIMIT 1) as message_id,
                'user' as role,
                substr(s.tool || ' session in ' || s.working_directory || COALESCE(' on branch ' || s.git_branch, ''), 1, 100) as snippet,
                s.started_at as timestamp,
                s.working_directory,
                s.tool,
                s.git_branch,
                s.message_count,
                s.started_at,
                0 as message_index
            FROM sessions s
            WHERE (
                s.tool LIKE ?{meta_param1}
                OR s.working_directory LIKE ?{meta_param2}
                OR s.git_branch LIKE ?{meta_param3}
            )
            "#
            ));

            // Add metadata patterns to params
            params_vec.push(Box::new(metadata_query_pattern.clone()));
            params_vec.push(Box::new(metadata_query_pattern.clone()));
            params_vec.push(Box::new(metadata_query_pattern));

            // Re-apply session-level filters to the UNION query
            if options.repo.is_some() {
                sql.push_str(&format!(" AND s.working_directory LIKE ?{param_idx}"));
                params_vec.push(Box::new(format!("{}%", options.repo.as_ref().unwrap())));
                param_idx += 1;
            }
            if options.tool.is_some() {
                sql.push_str(&format!(" AND LOWER(s.tool) = LOWER(?{param_idx})"));
                params_vec.push(Box::new(options.tool.as_ref().unwrap().clone()));
                param_idx += 1;
            }
            if options.since.is_some() {
                sql.push_str(&format!(" AND s.started_at >= ?{param_idx}"));
                params_vec.push(Box::new(options.since.unwrap().to_rfc3339()));
                param_idx += 1;
            }
            if options.until.is_some() {
                sql.push_str(&format!(" AND s.started_at <= ?{param_idx}"));
                params_vec.push(Box::new(options.until.unwrap().to_rfc3339()));
                param_idx += 1;
            }
            if options.project.is_some() {
                sql.push_str(&format!(" AND s.working_directory LIKE ?{param_idx}"));
                params_vec.push(Box::new(format!("%{}%", options.project.as_ref().unwrap())));
                param_idx += 1;
            }
            if options.branch.is_some() {
                sql.push_str(&format!(" AND s.git_branch LIKE ?{param_idx}"));
                params_vec.push(Box::new(format!("%{}%", options.branch.as_ref().unwrap())));
                param_idx += 1;
            }
        }

        sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT ?{param_idx}"));
        params_vec.push(Box::new(options.limit as i64));

        // Prepare and execute
        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            let role_str: String = row.get(2)?;
            let git_branch: Option<String> = row.get(7)?;
            let started_at_str: Option<String> = row.get(9)?;

            Ok(SearchResult {
                session_id: parse_uuid(&row.get::<_, String>(0)?)?,
                message_id: parse_uuid(&row.get::<_, String>(1)?)?,
                role: match role_str.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "system" => MessageRole::System,
                    _ => MessageRole::User,
                },
                snippet: row.get(3)?,
                timestamp: parse_datetime(&row.get::<_, String>(4)?)?,
                working_directory: row.get(5)?,
                tool: row.get(6)?,
                git_branch,
                session_message_count: row.get(8)?,
                session_started_at: started_at_str.map(|s| parse_datetime(&s)).transpose()?,
                message_index: row.get(10)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .context("Failed to search messages")
    }

    /// Gets messages around a specific message for context.
    ///
    /// Returns N messages before and N messages after the specified message,
    /// useful for displaying search results with surrounding context.
    pub fn get_context_messages(
        &self,
        session_id: &Uuid,
        message_index: i32,
        context_count: usize,
    ) -> Result<(Vec<Message>, Vec<Message>)> {
        // Get messages before
        let mut before_stmt = self.conn.prepare(
            "SELECT id, session_id, parent_id, idx, timestamp, role, content, model, git_branch, cwd
             FROM messages
             WHERE session_id = ?1 AND idx < ?2
             ORDER BY idx DESC
             LIMIT ?3",
        )?;

        let before_rows = before_stmt.query_map(
            params![session_id.to_string(), message_index, context_count as i64],
            Self::row_to_message,
        )?;

        let mut before: Vec<Message> = before_rows
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to get before messages")?;
        before.reverse(); // Put in chronological order

        // Get messages after
        let mut after_stmt = self.conn.prepare(
            "SELECT id, session_id, parent_id, idx, timestamp, role, content, model, git_branch, cwd
             FROM messages
             WHERE session_id = ?1 AND idx > ?2
             ORDER BY idx ASC
             LIMIT ?3",
        )?;

        let after_rows = after_stmt.query_map(
            params![session_id.to_string(), message_index, context_count as i64],
            Self::row_to_message,
        )?;

        let after: Vec<Message> = after_rows
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to get after messages")?;

        Ok((before, after))
    }

    /// Gets a single message by its index within a session.
    #[allow(dead_code)]
    pub fn get_message_by_index(&self, session_id: &Uuid, index: i32) -> Result<Option<Message>> {
        self.conn
            .query_row(
                "SELECT id, session_id, parent_id, idx, timestamp, role, content, model, git_branch, cwd
                 FROM messages
                 WHERE session_id = ?1 AND idx = ?2",
                params![session_id.to_string(), index],
                Self::row_to_message,
            )
            .optional()
            .context("Failed to get message by index")
    }

    fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
        let role_str: String = row.get(5)?;
        let content_str: String = row.get(6)?;

        let parent_id_str: Option<String> = row.get(2)?;
        let parent_id = match parent_id_str {
            Some(s) => Some(parse_uuid(&s)?),
            None => None,
        };

        Ok(Message {
            id: parse_uuid(&row.get::<_, String>(0)?)?,
            session_id: parse_uuid(&row.get::<_, String>(1)?)?,
            parent_id,
            index: row.get(3)?,
            timestamp: parse_datetime(&row.get::<_, String>(4)?)?,
            role: match role_str.as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "system" => MessageRole::System,
                _ => MessageRole::User,
            },
            content: serde_json::from_str(&content_str)
                .unwrap_or(MessageContent::Text(content_str)),
            model: row.get(7)?,
            git_branch: row.get(8)?,
            cwd: row.get(9)?,
        })
    }

    /// Rebuilds the full-text search index from existing messages and sessions.
    ///
    /// This should be called when:
    /// - Upgrading from a database without FTS support
    /// - The FTS index becomes corrupted or out of sync
    ///
    /// Returns the number of messages indexed.
    pub fn rebuild_search_index(&self) -> Result<usize> {
        // Clear existing FTS data
        self.conn.execute("DELETE FROM messages_fts", [])?;
        self.conn.execute("DELETE FROM sessions_fts", [])?;

        // Reindex all messages
        let mut msg_stmt = self.conn.prepare("SELECT id, content FROM messages")?;

        let rows = msg_stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let content_json: String = row.get(1)?;
            Ok((id, content_json))
        })?;

        let mut count = 0;
        for row in rows {
            let (id, content_json) = row?;
            // Parse the content JSON and extract text
            let content: MessageContent = serde_json::from_str(&content_json)
                .unwrap_or(MessageContent::Text(content_json.clone()));
            let text_content = content.text();

            if !text_content.is_empty() {
                self.conn.execute(
                    "INSERT INTO messages_fts (message_id, text_content) VALUES (?1, ?2)",
                    params![id, text_content],
                )?;
                count += 1;
            }
        }

        // Reindex all sessions for metadata search
        let mut session_stmt = self
            .conn
            .prepare("SELECT id, tool, working_directory, git_branch FROM sessions")?;

        let session_rows = session_stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let tool: String = row.get(1)?;
            let working_directory: String = row.get(2)?;
            let git_branch: Option<String> = row.get(3)?;
            Ok((id, tool, working_directory, git_branch))
        })?;

        for row in session_rows {
            let (id, tool, working_directory, git_branch) = row?;
            self.conn.execute(
                "INSERT INTO sessions_fts (session_id, tool, working_directory, git_branch) VALUES (?1, ?2, ?3, ?4)",
                params![id, tool, working_directory, git_branch.unwrap_or_default()],
            )?;
        }

        Ok(count)
    }

    /// Checks if the search index needs rebuilding.
    ///
    /// Returns true if there are messages or sessions in the database but the FTS
    /// indexes are empty, indicating data was imported before FTS was added.
    pub fn search_index_needs_rebuild(&self) -> Result<bool> {
        let message_count: i32 =
            self.conn
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;

        let msg_fts_count: i32 =
            self.conn
                .query_row("SELECT COUNT(*) FROM messages_fts", [], |row| row.get(0))?;

        let session_count: i32 =
            self.conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

        let session_fts_count: i32 =
            self.conn
                .query_row("SELECT COUNT(*) FROM sessions_fts", [], |row| row.get(0))?;

        // Rebuild needed if we have messages/sessions but either FTS index is empty
        Ok((message_count > 0 && msg_fts_count == 0)
            || (session_count > 0 && session_fts_count == 0))
    }

    // ==================== Stats ====================

    /// Returns the total number of sessions in the database.
    pub fn session_count(&self) -> Result<i32> {
        let count: i32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Returns the total number of messages across all sessions.
    pub fn message_count(&self) -> Result<i32> {
        let count: i32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Returns the total number of session links in the database.
    pub fn link_count(&self) -> Result<i32> {
        let count: i32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM session_links", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Returns the path to the database file, if available.
    ///
    /// Returns `None` for in-memory databases.
    pub fn db_path(&self) -> Option<std::path::PathBuf> {
        self.conn.path().map(std::path::PathBuf::from)
    }

    // ==================== Auto-linking ====================

    /// Finds sessions that were active around a commit time.
    ///
    /// A session is considered active if the commit time falls within the
    /// window before and after the session's time range (started_at to ended_at).
    ///
    /// # Arguments
    ///
    /// * `commit_time` - The timestamp of the commit
    /// * `window_minutes` - The window in minutes before/after the session
    /// * `working_dir` - Optional working directory filter (prefix match)
    ///
    /// # Returns
    ///
    /// Sessions that were active near the commit time, ordered by proximity.
    pub fn find_sessions_near_commit_time(
        &self,
        commit_time: chrono::DateTime<chrono::Utc>,
        window_minutes: i64,
        working_dir: Option<&str>,
    ) -> Result<Vec<Session>> {
        // Convert commit time to RFC3339 for SQLite comparison
        let commit_time_str = commit_time.to_rfc3339();

        // Calculate the time window boundaries
        let window = chrono::Duration::minutes(window_minutes);
        let window_start = (commit_time - window).to_rfc3339();
        let window_end = (commit_time + window).to_rfc3339();

        let sql = if working_dir.is_some() {
            r#"
            SELECT id, tool, tool_version, started_at, ended_at, model,
                   working_directory, git_branch, source_path, message_count
            FROM sessions
            WHERE working_directory LIKE ?1
              AND (
                  -- Session started before or during the window
                  (started_at <= ?3)
                  AND
                  -- Session ended after or during the window (or is still ongoing)
                  (ended_at IS NULL OR ended_at >= ?2)
              )
            ORDER BY
              -- Order by how close the session end (or start) is to commit time
              ABS(julianday(COALESCE(ended_at, started_at)) - julianday(?4))
            "#
        } else {
            r#"
            SELECT id, tool, tool_version, started_at, ended_at, model,
                   working_directory, git_branch, source_path, message_count
            FROM sessions
            WHERE
              -- Session started before or during the window
              (started_at <= ?2)
              AND
              -- Session ended after or during the window (or is still ongoing)
              (ended_at IS NULL OR ended_at >= ?1)
            ORDER BY
              -- Order by how close the session end (or start) is to commit time
              ABS(julianday(COALESCE(ended_at, started_at)) - julianday(?3))
            "#
        };

        let mut stmt = self.conn.prepare(sql)?;

        let rows = if let Some(wd) = working_dir {
            stmt.query_map(
                params![format!("{wd}%"), window_start, window_end, commit_time_str],
                Self::row_to_session,
            )?
        } else {
            stmt.query_map(
                params![window_start, window_end, commit_time_str],
                Self::row_to_session,
            )?
        };

        rows.collect::<Result<Vec<_>, _>>()
            .context("Failed to find sessions near commit time")
    }

    /// Checks if a link already exists between a session and commit.
    ///
    /// Used to avoid creating duplicate links during auto-linking.
    pub fn link_exists(&self, session_id: &Uuid, commit_sha: &str) -> Result<bool> {
        let pattern = format!("{commit_sha}%");
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM session_links WHERE session_id = ?1 AND commit_sha LIKE ?2",
            params![session_id.to_string(), pattern],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    // ==================== Session Deletion ====================

    /// Deletes a session and all its associated data.
    ///
    /// Removes the session, all its messages, all FTS index entries, and all
    /// session links. Returns the counts of deleted items.
    ///
    /// # Returns
    ///
    /// A tuple of (messages_deleted, links_deleted) counts.
    pub fn delete_session(&self, session_id: &Uuid) -> Result<(usize, usize)> {
        let session_id_str = session_id.to_string();

        // Delete from messages_fts first (need message IDs)
        self.conn.execute(
            "DELETE FROM messages_fts WHERE message_id IN (SELECT id FROM messages WHERE session_id = ?1)",
            params![session_id_str],
        )?;

        // Delete messages
        let messages_deleted = self.conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id_str],
        )?;

        // Delete links
        let links_deleted = self.conn.execute(
            "DELETE FROM session_links WHERE session_id = ?1",
            params![session_id_str],
        )?;

        // Delete from sessions_fts
        self.conn.execute(
            "DELETE FROM sessions_fts WHERE session_id = ?1",
            params![session_id_str],
        )?;

        // Delete the session itself
        self.conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            params![session_id_str],
        )?;

        Ok((messages_deleted, links_deleted))
    }

    // ==================== Database Maintenance ====================

    /// Runs SQLite VACUUM to reclaim unused space and defragment the database.
    ///
    /// This operation can take some time on large databases and temporarily
    /// doubles the disk space used while rebuilding.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute("VACUUM", [])?;
        Ok(())
    }

    /// Returns the file size of the database in bytes.
    ///
    /// Returns `None` for in-memory databases.
    pub fn file_size(&self) -> Result<Option<u64>> {
        if let Some(path) = self.db_path() {
            let metadata = std::fs::metadata(&path)?;
            Ok(Some(metadata.len()))
        } else {
            Ok(None)
        }
    }

    /// Deletes sessions older than the specified date.
    ///
    /// Also deletes all associated messages, links, and FTS entries.
    ///
    /// # Arguments
    ///
    /// * `before` - Delete sessions that started before this date
    ///
    /// # Returns
    ///
    /// The number of sessions deleted.
    pub fn delete_sessions_older_than(&self, before: DateTime<Utc>) -> Result<usize> {
        let before_str = before.to_rfc3339();

        // Get session IDs to delete
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sessions WHERE started_at < ?1")?;
        let session_ids: Vec<String> = stmt
            .query_map(params![before_str], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        if session_ids.is_empty() {
            return Ok(0);
        }

        let count = session_ids.len();

        // Delete associated data for each session
        for session_id_str in &session_ids {
            // Delete from messages_fts
            self.conn.execute(
                "DELETE FROM messages_fts WHERE message_id IN (SELECT id FROM messages WHERE session_id = ?1)",
                params![session_id_str],
            )?;

            // Delete messages
            self.conn.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params![session_id_str],
            )?;

            // Delete links
            self.conn.execute(
                "DELETE FROM session_links WHERE session_id = ?1",
                params![session_id_str],
            )?;

            // Delete from sessions_fts
            self.conn.execute(
                "DELETE FROM sessions_fts WHERE session_id = ?1",
                params![session_id_str],
            )?;
        }

        // Delete the sessions
        self.conn.execute(
            "DELETE FROM sessions WHERE started_at < ?1",
            params![before_str],
        )?;

        Ok(count)
    }

    /// Counts sessions older than the specified date (for dry-run preview).
    ///
    /// # Arguments
    ///
    /// * `before` - Count sessions that started before this date
    ///
    /// # Returns
    ///
    /// The number of sessions that would be deleted.
    pub fn count_sessions_older_than(&self, before: DateTime<Utc>) -> Result<i32> {
        let before_str = before.to_rfc3339();
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE started_at < ?1",
            params![before_str],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Returns database statistics including counts and date ranges.
    ///
    /// # Returns
    ///
    /// A `DatabaseStats` struct with session, message, and link counts,
    /// plus the date range of sessions and a breakdown by tool.
    pub fn stats(&self) -> Result<DatabaseStats> {
        let session_count = self.session_count()?;
        let message_count = self.message_count()?;
        let link_count = self.link_count()?;

        // Get date range
        let oldest: Option<String> = self
            .conn
            .query_row("SELECT MIN(started_at) FROM sessions", [], |row| row.get(0))
            .optional()?
            .flatten();

        let newest: Option<String> = self
            .conn
            .query_row("SELECT MAX(started_at) FROM sessions", [], |row| row.get(0))
            .optional()?
            .flatten();

        let oldest_session = oldest
            .map(|s| parse_datetime(&s))
            .transpose()
            .unwrap_or(None);
        let newest_session = newest
            .map(|s| parse_datetime(&s))
            .transpose()
            .unwrap_or(None);

        // Get sessions by tool
        let mut stmt = self
            .conn
            .prepare("SELECT tool, COUNT(*) FROM sessions GROUP BY tool ORDER BY COUNT(*) DESC")?;
        let sessions_by_tool: Vec<(String, i32)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(DatabaseStats {
            session_count,
            message_count,
            link_count,
            oldest_session,
            newest_session,
            sessions_by_tool,
        })
    }
}

/// Statistics about the Lore database.
#[derive(Debug, Clone)]
pub struct DatabaseStats {
    /// Total number of sessions.
    pub session_count: i32,
    /// Total number of messages.
    pub message_count: i32,
    /// Total number of session links.
    pub link_count: i32,
    /// Timestamp of the oldest session.
    pub oldest_session: Option<DateTime<Utc>>,
    /// Timestamp of the newest session.
    pub newest_session: Option<DateTime<Utc>>,
    /// Session counts grouped by tool name.
    pub sessions_by_tool: Vec<(String, i32)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{
        LinkCreator, LinkType, MessageContent, MessageRole, SearchOptions,
    };
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
        let session1 =
            create_test_session("claude-code", "/project1", now - Duration::hours(2), None);
        let session2 = create_test_session("cursor", "/project2", now - Duration::hours(1), None);
        let session3 = create_test_session("claude-code", "/project3", now, None);

        db.insert_session(&session1)
            .expect("Failed to insert session1");
        db.insert_session(&session2)
            .expect("Failed to insert session2");
        db.insert_session(&session3)
            .expect("Failed to insert session3");

        let sessions = db.list_sessions(10, None).expect("Failed to list sessions");

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
        assert_eq!(sessions[2].id, session1.id, "Oldest session should be last");
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
        let session2 = create_test_session("claude-code", "/home/user/project-b", now, None);
        let session3 = create_test_session("claude-code", "/other/path", now, None);

        db.insert_session(&session1)
            .expect("Failed to insert session1");
        db.insert_session(&session2)
            .expect("Failed to insert session2");
        db.insert_session(&session3)
            .expect("Failed to insert session3");

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
        assert!(ids.contains(&session1.id), "Should contain session1");
        assert!(ids.contains(&session2.id), "Should contain session2");
        assert!(!ids.contains(&session3.id), "Should not contain session3");
    }

    #[test]
    fn test_session_exists_by_source() {
        let (db, _dir) = create_test_db();
        let source_path = "/path/to/session.jsonl";

        let session = create_test_session("claude-code", "/project", Utc::now(), Some(source_path));

        // Before insert, should not exist
        assert!(
            !db.session_exists_by_source(source_path)
                .expect("Failed to check existence"),
            "Session should not exist before insert"
        );

        db.insert_session(&session)
            .expect("Failed to insert session");

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
        db.insert_session(&session)
            .expect("Failed to insert session");

        let msg1 = create_test_message(session.id, 0, MessageRole::User, "Hello");
        let msg2 = create_test_message(session.id, 1, MessageRole::Assistant, "Hi there!");

        db.insert_message(&msg1)
            .expect("Failed to insert message 1");
        db.insert_message(&msg2)
            .expect("Failed to insert message 2");

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
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Insert messages out of order
        let msg3 = create_test_message(session.id, 2, MessageRole::Assistant, "Third");
        let msg1 = create_test_message(session.id, 0, MessageRole::User, "First");
        let msg2 = create_test_message(session.id, 1, MessageRole::Assistant, "Second");

        db.insert_message(&msg3)
            .expect("Failed to insert message 3");
        db.insert_message(&msg1)
            .expect("Failed to insert message 1");
        db.insert_message(&msg2)
            .expect("Failed to insert message 2");

        let messages = db
            .get_messages(&session.id)
            .expect("Failed to get messages");

        assert_eq!(messages.len(), 3, "Should have 3 messages");
        assert_eq!(messages[0].index, 0, "First message should have index 0");
        assert_eq!(messages[1].index, 1, "Second message should have index 1");
        assert_eq!(messages[2].index, 2, "Third message should have index 2");

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
        db.insert_session(&session)
            .expect("Failed to insert session");

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
        db.insert_session(&session)
            .expect("Failed to insert session");

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
        db.insert_session(&session1)
            .expect("Failed to insert session1");

        assert_eq!(
            db.session_count().expect("Failed to get count"),
            1,
            "Session count should be 1 after first insert"
        );

        let session2 = create_test_session("cursor", "/project2", Utc::now(), None);
        db.insert_session(&session2)
            .expect("Failed to insert session2");

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
        db.insert_session(&session)
            .expect("Failed to insert session");

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

    #[test]
    fn test_link_count() {
        let (db, _dir) = create_test_db();

        assert_eq!(
            db.link_count().expect("Failed to get count"),
            0,
            "Initial link count should be 0"
        );

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let link1 = create_test_link(session.id, Some("abc123def456"), LinkType::Commit);
        db.insert_link(&link1).expect("Failed to insert link1");

        assert_eq!(
            db.link_count().expect("Failed to get count"),
            1,
            "Link count should be 1 after first insert"
        );

        let link2 = create_test_link(session.id, Some("def456abc789"), LinkType::Commit);
        db.insert_link(&link2).expect("Failed to insert link2");

        assert_eq!(
            db.link_count().expect("Failed to get count"),
            2,
            "Link count should be 2 after second insert"
        );
    }

    #[test]
    fn test_db_path() {
        let dir = tempdir().expect("Failed to create temp directory");
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).expect("Failed to open test database");

        let retrieved_path = db.db_path();
        assert!(
            retrieved_path.is_some(),
            "Database path should be available"
        );

        // Canonicalize both paths to handle macOS /var -> /private/var symlinks
        let expected = db_path.canonicalize().unwrap_or(db_path);
        let actual = retrieved_path.unwrap();
        let actual_canonical = actual.canonicalize().unwrap_or(actual.clone());

        assert_eq!(
            actual_canonical, expected,
            "Database path should match (after canonicalization)"
        );
    }

    // ==================== Search Tests ====================

    #[test]
    fn test_search_messages_basic() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/home/user/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let msg1 = create_test_message(
            session.id,
            0,
            MessageRole::User,
            "How do I implement error handling in Rust?",
        );
        let msg2 = create_test_message(
            session.id,
            1,
            MessageRole::Assistant,
            "You can use Result types for error handling. The anyhow crate is also helpful.",
        );

        db.insert_message(&msg1)
            .expect("Failed to insert message 1");
        db.insert_message(&msg2)
            .expect("Failed to insert message 2");

        // Search for "error"
        let results = db
            .search_messages("error", 10, None, None, None)
            .expect("Failed to search");

        assert_eq!(
            results.len(),
            2,
            "Should find 2 messages containing 'error'"
        );
    }

    #[test]
    fn test_search_messages_no_results() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let msg = create_test_message(session.id, 0, MessageRole::User, "Hello world");
        db.insert_message(&msg).expect("Failed to insert message");

        // Search for something not in the messages
        let results = db
            .search_messages("nonexistent_term_xyz", 10, None, None, None)
            .expect("Failed to search");

        assert!(results.is_empty(), "Should find no results");
    }

    #[test]
    fn test_search_messages_with_role_filter() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let msg1 = create_test_message(
            session.id,
            0,
            MessageRole::User,
            "Tell me about Rust programming",
        );
        let msg2 = create_test_message(
            session.id,
            1,
            MessageRole::Assistant,
            "Rust is a systems programming language",
        );

        db.insert_message(&msg1)
            .expect("Failed to insert message 1");
        db.insert_message(&msg2)
            .expect("Failed to insert message 2");

        // Search with user role filter
        let user_results = db
            .search_messages("programming", 10, None, None, Some("user"))
            .expect("Failed to search");

        assert_eq!(user_results.len(), 1, "Should find 1 user message");
        assert_eq!(
            user_results[0].role,
            MessageRole::User,
            "Result should be from user"
        );

        // Search with assistant role filter
        let assistant_results = db
            .search_messages("programming", 10, None, None, Some("assistant"))
            .expect("Failed to search");

        assert_eq!(
            assistant_results.len(),
            1,
            "Should find 1 assistant message"
        );
        assert_eq!(
            assistant_results[0].role,
            MessageRole::Assistant,
            "Result should be from assistant"
        );
    }

    #[test]
    fn test_search_messages_with_repo_filter() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/home/user/project-a", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/home/user/project-b", Utc::now(), None);

        db.insert_session(&session1).expect("insert 1");
        db.insert_session(&session2).expect("insert 2");

        let msg1 = create_test_message(session1.id, 0, MessageRole::User, "Hello from project-a");
        let msg2 = create_test_message(session2.id, 0, MessageRole::User, "Hello from project-b");

        db.insert_message(&msg1).expect("insert msg 1");
        db.insert_message(&msg2).expect("insert msg 2");

        // Search with repo filter
        let results = db
            .search_messages("Hello", 10, Some("/home/user/project-a"), None, None)
            .expect("Failed to search");

        assert_eq!(results.len(), 1, "Should find 1 message in project-a");
        assert!(
            results[0].working_directory.contains("project-a"),
            "Should be from project-a"
        );
    }

    #[test]
    fn test_search_messages_limit() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        // Insert 5 messages all containing "test"
        for i in 0..5 {
            let msg = create_test_message(
                session.id,
                i,
                MessageRole::User,
                &format!("This is test message number {i}"),
            );
            db.insert_message(&msg).expect("insert message");
        }

        // Search with limit of 3
        let results = db
            .search_messages("test", 3, None, None, None)
            .expect("Failed to search");

        assert_eq!(results.len(), 3, "Should respect limit of 3");
    }

    #[test]
    fn test_search_index_needs_rebuild_empty_db() {
        let (db, _dir) = create_test_db();

        let needs_rebuild = db
            .search_index_needs_rebuild()
            .expect("Failed to check rebuild status");

        assert!(!needs_rebuild, "Empty database should not need rebuild");
    }

    #[test]
    fn test_rebuild_search_index() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        let msg1 = create_test_message(session.id, 0, MessageRole::User, "First test message");
        let msg2 = create_test_message(
            session.id,
            1,
            MessageRole::Assistant,
            "Second test response",
        );

        db.insert_message(&msg1).expect("insert msg 1");
        db.insert_message(&msg2).expect("insert msg 2");

        // Clear and rebuild the index
        db.conn
            .execute("DELETE FROM messages_fts", [])
            .expect("clear fts");

        // Index should now need rebuilding
        assert!(
            db.search_index_needs_rebuild().expect("check rebuild"),
            "Should need rebuild after clearing FTS"
        );

        // Rebuild
        let count = db.rebuild_search_index().expect("rebuild");
        assert_eq!(count, 2, "Should have indexed 2 messages");

        // Index should no longer need rebuilding
        assert!(
            !db.search_index_needs_rebuild().expect("check rebuild"),
            "Should not need rebuild after rebuilding"
        );

        // Search should work
        let results = db
            .search_messages("test", 10, None, None, None)
            .expect("search");
        assert_eq!(results.len(), 2, "Should find 2 results after rebuild");
    }

    #[test]
    fn test_search_with_block_content() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        // Create a message with block content
        let block_content = MessageContent::Blocks(vec![
            crate::storage::models::ContentBlock::Text {
                text: "Let me help with your database query.".to_string(),
            },
            crate::storage::models::ContentBlock::ToolUse {
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

        // Search should find text from blocks
        let results = db
            .search_messages("database", 10, None, None, None)
            .expect("search");

        assert_eq!(results.len(), 1, "Should find message with block content");
    }

    #[test]
    fn test_search_result_contains_session_info() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/home/user/my-project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        let msg = create_test_message(session.id, 0, MessageRole::User, "Search test message");
        db.insert_message(&msg).expect("insert message");

        let results = db
            .search_messages("Search", 10, None, None, None)
            .expect("search");

        assert_eq!(results.len(), 1, "Should find 1 result");
        assert_eq!(results[0].session_id, session.id, "Session ID should match");
        assert_eq!(results[0].message_id, msg.id, "Message ID should match");
        assert_eq!(
            results[0].working_directory, "/home/user/my-project",
            "Working directory should match"
        );
        assert_eq!(results[0].role, MessageRole::User, "Role should match");
    }

    // ==================== Delete Link Tests ====================

    #[test]
    fn test_delete_link_by_id() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let link = create_test_link(session.id, Some("abc123def456"), LinkType::Commit);
        db.insert_link(&link).expect("Failed to insert link");

        // Verify link exists
        let links_before = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links_before.len(), 1, "Should have 1 link before delete");

        // Delete the link
        let deleted = db.delete_link(&link.id).expect("Failed to delete link");
        assert!(deleted, "Should return true when link is deleted");

        // Verify link is gone
        let links_after = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links_after.len(), 0, "Should have 0 links after delete");
    }

    #[test]
    fn test_delete_link_nonexistent() {
        let (db, _dir) = create_test_db();

        let nonexistent_id = Uuid::new_v4();
        let deleted = db
            .delete_link(&nonexistent_id)
            .expect("Failed to call delete_link");

        assert!(!deleted, "Should return false for nonexistent link");
    }

    #[test]
    fn test_delete_links_by_session() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Create multiple links for the same session
        let link1 = create_test_link(session.id, Some("abc123"), LinkType::Commit);
        let link2 = create_test_link(session.id, Some("def456"), LinkType::Commit);
        let link3 = create_test_link(session.id, Some("ghi789"), LinkType::Commit);

        db.insert_link(&link1).expect("Failed to insert link1");
        db.insert_link(&link2).expect("Failed to insert link2");
        db.insert_link(&link3).expect("Failed to insert link3");

        // Verify all links exist
        let links_before = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links_before.len(), 3, "Should have 3 links before delete");

        // Delete all links for the session
        let count = db
            .delete_links_by_session(&session.id)
            .expect("Failed to delete links");
        assert_eq!(count, 3, "Should have deleted 3 links");

        // Verify all links are gone
        let links_after = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links_after.len(), 0, "Should have 0 links after delete");
    }

    #[test]
    fn test_delete_links_by_session_no_links() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        // Delete links for session that has none
        let count = db
            .delete_links_by_session(&session.id)
            .expect("Failed to call delete_links_by_session");
        assert_eq!(count, 0, "Should return 0 when no links exist");
    }

    #[test]
    fn test_delete_links_by_session_preserves_other_sessions() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/project2", Utc::now(), None);

        db.insert_session(&session1)
            .expect("Failed to insert session1");
        db.insert_session(&session2)
            .expect("Failed to insert session2");

        let link1 = create_test_link(session1.id, Some("abc123"), LinkType::Commit);
        let link2 = create_test_link(session2.id, Some("def456"), LinkType::Commit);

        db.insert_link(&link1).expect("Failed to insert link1");
        db.insert_link(&link2).expect("Failed to insert link2");

        // Delete links only for session1
        let count = db
            .delete_links_by_session(&session1.id)
            .expect("Failed to delete links");
        assert_eq!(count, 1, "Should have deleted 1 link");

        // Verify session2's link is preserved
        let session2_links = db
            .get_links_by_session(&session2.id)
            .expect("Failed to get links");
        assert_eq!(
            session2_links.len(),
            1,
            "Session2's link should be preserved"
        );
        assert_eq!(session2_links[0].id, link2.id, "Link ID should match");
    }

    #[test]
    fn test_delete_link_by_session_and_commit() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let link1 = create_test_link(session.id, Some("abc123def456"), LinkType::Commit);
        let link2 = create_test_link(session.id, Some("def456abc789"), LinkType::Commit);

        db.insert_link(&link1).expect("Failed to insert link1");
        db.insert_link(&link2).expect("Failed to insert link2");

        // Delete only the first link by commit SHA
        let deleted = db
            .delete_link_by_session_and_commit(&session.id, "abc123")
            .expect("Failed to delete link");
        assert!(deleted, "Should return true when link is deleted");

        // Verify only link2 remains
        let links = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links.len(), 1, "Should have 1 link remaining");
        assert_eq!(links[0].id, link2.id, "Remaining link should be link2");
    }

    #[test]
    fn test_delete_link_by_session_and_commit_full_sha() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let full_sha = "abc123def456789012345678901234567890abcd";
        let link = create_test_link(session.id, Some(full_sha), LinkType::Commit);
        db.insert_link(&link).expect("Failed to insert link");

        // Delete using full SHA
        let deleted = db
            .delete_link_by_session_and_commit(&session.id, full_sha)
            .expect("Failed to delete link");
        assert!(deleted, "Should delete with full SHA");

        let links = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links.len(), 0, "Should have 0 links after delete");
    }

    #[test]
    fn test_delete_link_by_session_and_commit_no_match() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session)
            .expect("Failed to insert session");

        let link = create_test_link(session.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("Failed to insert link");

        // Try to delete with non-matching commit
        let deleted = db
            .delete_link_by_session_and_commit(&session.id, "xyz999")
            .expect("Failed to call delete");
        assert!(!deleted, "Should return false when no match");

        // Verify original link is preserved
        let links = db
            .get_links_by_session(&session.id)
            .expect("Failed to get links");
        assert_eq!(links.len(), 1, "Link should be preserved");
    }

    #[test]
    fn test_delete_link_by_session_and_commit_wrong_session() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/project2", Utc::now(), None);

        db.insert_session(&session1)
            .expect("Failed to insert session1");
        db.insert_session(&session2)
            .expect("Failed to insert session2");

        let link = create_test_link(session1.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("Failed to insert link");

        // Try to delete from wrong session
        let deleted = db
            .delete_link_by_session_and_commit(&session2.id, "abc123")
            .expect("Failed to call delete");
        assert!(!deleted, "Should not delete link from different session");

        // Verify original link is preserved
        let links = db
            .get_links_by_session(&session1.id)
            .expect("Failed to get links");
        assert_eq!(links.len(), 1, "Link should be preserved");
    }

    // ==================== Auto-linking Tests ====================

    #[test]
    fn test_find_sessions_near_commit_time_basic() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create a session that ended 10 minutes ago
        let mut session = create_test_session(
            "claude-code",
            "/home/user/project",
            now - Duration::hours(1),
            None,
        );
        session.ended_at = Some(now - Duration::minutes(10));

        db.insert_session(&session).expect("insert session");

        // Find sessions near "now" with a 30 minute window
        let found = db
            .find_sessions_near_commit_time(now, 30, None)
            .expect("find sessions");

        assert_eq!(found.len(), 1, "Should find session within window");
        assert_eq!(found[0].id, session.id);
    }

    #[test]
    fn test_find_sessions_near_commit_time_outside_window() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create a session that ended 2 hours ago
        let mut session =
            create_test_session("claude-code", "/project", now - Duration::hours(3), None);
        session.ended_at = Some(now - Duration::hours(2));

        db.insert_session(&session).expect("insert session");

        // Find sessions near "now" with a 30 minute window
        let found = db
            .find_sessions_near_commit_time(now, 30, None)
            .expect("find sessions");

        assert!(found.is_empty(), "Should not find session outside window");
    }

    #[test]
    fn test_find_sessions_near_commit_time_with_working_dir() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create sessions in different directories
        let mut session1 = create_test_session(
            "claude-code",
            "/home/user/project-a",
            now - Duration::minutes(30),
            None,
        );
        session1.ended_at = Some(now - Duration::minutes(5));

        let mut session2 = create_test_session(
            "claude-code",
            "/home/user/project-b",
            now - Duration::minutes(30),
            None,
        );
        session2.ended_at = Some(now - Duration::minutes(5));

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        // Find sessions near "now" filtering by project-a
        let found = db
            .find_sessions_near_commit_time(now, 30, Some("/home/user/project-a"))
            .expect("find sessions");

        assert_eq!(found.len(), 1, "Should find only session in project-a");
        assert_eq!(found[0].id, session1.id);
    }

    #[test]
    fn test_find_sessions_near_commit_time_ongoing_session() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create an ongoing session (no ended_at)
        let session =
            create_test_session("claude-code", "/project", now - Duration::minutes(20), None);
        // ended_at is None by default

        db.insert_session(&session).expect("insert session");

        // Find sessions near "now"
        let found = db
            .find_sessions_near_commit_time(now, 30, None)
            .expect("find sessions");

        assert_eq!(found.len(), 1, "Should find ongoing session");
        assert_eq!(found[0].id, session.id);
    }

    #[test]
    fn test_link_exists_true() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        let link = create_test_link(session.id, Some("abc123def456"), LinkType::Commit);
        db.insert_link(&link).expect("insert link");

        // Check with full SHA
        assert!(
            db.link_exists(&session.id, "abc123def456")
                .expect("check exists"),
            "Should find link with full SHA"
        );

        // Check with partial SHA
        assert!(
            db.link_exists(&session.id, "abc123").expect("check exists"),
            "Should find link with partial SHA"
        );
    }

    #[test]
    fn test_link_exists_false() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        // No links created
        assert!(
            !db.link_exists(&session.id, "abc123").expect("check exists"),
            "Should not find non-existent link"
        );
    }

    #[test]
    fn test_link_exists_different_session() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("claude-code", "/project2", Utc::now(), None);

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let link = create_test_link(session1.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("insert link");

        // Link exists for session1 but not session2
        assert!(
            db.link_exists(&session1.id, "abc123").expect("check"),
            "Should find link for session1"
        );
        assert!(
            !db.link_exists(&session2.id, "abc123").expect("check"),
            "Should not find link for session2"
        );
    }

    // ==================== Enhanced Search Tests ====================

    #[test]
    fn test_search_with_tool_filter() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("aider", "/project2", Utc::now(), None);

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let msg1 = create_test_message(session1.id, 0, MessageRole::User, "Hello from Claude");
        let msg2 = create_test_message(session2.id, 0, MessageRole::User, "Hello from Aider");

        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Search with tool filter
        let options = SearchOptions {
            query: "Hello".to_string(),
            limit: 10,
            tool: Some("claude-code".to_string()),
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        assert_eq!(results.len(), 1, "Should find 1 result with tool filter");
        assert_eq!(results[0].tool, "claude-code", "Should be from claude-code");
    }

    #[test]
    fn test_search_with_date_range() {
        let (db, _dir) = create_test_db();

        let old_time = Utc::now() - chrono::Duration::days(30);
        let new_time = Utc::now() - chrono::Duration::days(1);

        let session1 = create_test_session("claude-code", "/project1", old_time, None);
        let session2 = create_test_session("claude-code", "/project2", new_time, None);

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let msg1 = create_test_message(session1.id, 0, MessageRole::User, "Old session message");
        let msg2 = create_test_message(session2.id, 0, MessageRole::User, "New session message");

        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Search with since filter (last 7 days)
        let since = Utc::now() - chrono::Duration::days(7);
        let options = SearchOptions {
            query: "session".to_string(),
            limit: 10,
            since: Some(since),
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        assert_eq!(results.len(), 1, "Should find 1 result within date range");
        assert!(
            results[0].working_directory.contains("project2"),
            "Should be from newer project"
        );
    }

    #[test]
    fn test_search_with_project_filter() {
        let (db, _dir) = create_test_db();

        let session1 =
            create_test_session("claude-code", "/home/user/frontend-app", Utc::now(), None);
        let session2 =
            create_test_session("claude-code", "/home/user/backend-api", Utc::now(), None);

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let msg1 = create_test_message(session1.id, 0, MessageRole::User, "Testing frontend");
        let msg2 = create_test_message(session2.id, 0, MessageRole::User, "Testing backend");

        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Search with project filter
        let options = SearchOptions {
            query: "Testing".to_string(),
            limit: 10,
            project: Some("frontend".to_string()),
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        assert_eq!(results.len(), 1, "Should find 1 result with project filter");
        assert!(
            results[0].working_directory.contains("frontend"),
            "Should be from frontend project"
        );
    }

    #[test]
    fn test_search_with_branch_filter() {
        let (db, _dir) = create_test_db();

        let session1 = Session {
            id: Uuid::new_v4(),
            tool: "claude-code".to_string(),
            tool_version: None,
            started_at: Utc::now(),
            ended_at: None,
            model: None,
            working_directory: "/project".to_string(),
            git_branch: Some("feat/auth".to_string()),
            source_path: None,
            message_count: 0,
        };
        let session2 = Session {
            id: Uuid::new_v4(),
            tool: "claude-code".to_string(),
            tool_version: None,
            started_at: Utc::now(),
            ended_at: None,
            model: None,
            working_directory: "/project".to_string(),
            git_branch: Some("main".to_string()),
            source_path: None,
            message_count: 0,
        };

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let msg1 = create_test_message(session1.id, 0, MessageRole::User, "Auth feature work");
        let msg2 = create_test_message(session2.id, 0, MessageRole::User, "Main branch work");

        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Search with branch filter
        let options = SearchOptions {
            query: "work".to_string(),
            limit: 10,
            branch: Some("auth".to_string()),
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        assert_eq!(results.len(), 1, "Should find 1 result with branch filter");
        assert_eq!(
            results[0].git_branch.as_deref(),
            Some("feat/auth"),
            "Should be from feat/auth branch"
        );
    }

    #[test]
    fn test_search_metadata_matches_project() {
        let (db, _dir) = create_test_db();

        let session =
            create_test_session("claude-code", "/home/user/redactyl-app", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        // Add a message that does NOT contain "redactyl"
        let msg = create_test_message(session.id, 0, MessageRole::User, "Working on the project");
        db.insert_message(&msg).expect("insert msg");

        // Search for "redactyl" - should match session metadata
        let options = SearchOptions {
            query: "redactyl".to_string(),
            limit: 10,
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        assert_eq!(
            results.len(),
            1,
            "Should find session via metadata match on project name"
        );
    }

    #[test]
    fn test_search_returns_extended_session_info() {
        let (db, _dir) = create_test_db();

        let started_at = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            tool: "claude-code".to_string(),
            tool_version: Some("1.0.0".to_string()),
            started_at,
            ended_at: None,
            model: None,
            working_directory: "/home/user/myapp".to_string(),
            git_branch: Some("develop".to_string()),
            source_path: None,
            message_count: 5,
        };
        db.insert_session(&session).expect("insert session");

        let msg = create_test_message(session.id, 0, MessageRole::User, "Test message for search");
        db.insert_message(&msg).expect("insert msg");

        let options = SearchOptions {
            query: "Test".to_string(),
            limit: 10,
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        assert_eq!(results.len(), 1, "Should find 1 result");
        let result = &results[0];

        assert_eq!(result.tool, "claude-code", "Tool should be populated");
        assert_eq!(
            result.git_branch.as_deref(),
            Some("develop"),
            "Branch should be populated"
        );
        assert!(
            result.session_message_count > 0,
            "Message count should be populated"
        );
        assert!(
            result.session_started_at.is_some(),
            "Session start time should be populated"
        );
    }

    #[test]
    fn test_get_context_messages() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        // Create 5 messages in sequence
        for i in 0..5 {
            let role = if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            let msg = create_test_message(session.id, i, role, &format!("Message number {i}"));
            db.insert_message(&msg).expect("insert message");
        }

        // Get context around message index 2 (the middle one)
        let (before, after) = db
            .get_context_messages(&session.id, 2, 1)
            .expect("get context");

        assert_eq!(before.len(), 1, "Should have 1 message before");
        assert_eq!(after.len(), 1, "Should have 1 message after");
        assert_eq!(before[0].index, 1, "Before message should be index 1");
        assert_eq!(after[0].index, 3, "After message should be index 3");
    }

    #[test]
    fn test_get_context_messages_at_start() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        for i in 0..3 {
            let msg =
                create_test_message(session.id, i, MessageRole::User, &format!("Message {i}"));
            db.insert_message(&msg).expect("insert message");
        }

        // Get context around first message (index 0)
        let (before, after) = db
            .get_context_messages(&session.id, 0, 2)
            .expect("get context");

        assert!(
            before.is_empty(),
            "Should have no messages before first message"
        );
        assert_eq!(after.len(), 2, "Should have 2 messages after");
    }

    #[test]
    fn test_get_context_messages_at_end() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        for i in 0..3 {
            let msg =
                create_test_message(session.id, i, MessageRole::User, &format!("Message {i}"));
            db.insert_message(&msg).expect("insert message");
        }

        // Get context around last message (index 2)
        let (before, after) = db
            .get_context_messages(&session.id, 2, 2)
            .expect("get context");

        assert_eq!(before.len(), 2, "Should have 2 messages before");
        assert!(
            after.is_empty(),
            "Should have no messages after last message"
        );
    }

    #[test]
    fn test_search_combined_filters() {
        let (db, _dir) = create_test_db();

        let session1 = Session {
            id: Uuid::new_v4(),
            tool: "claude-code".to_string(),
            tool_version: None,
            started_at: Utc::now(),
            ended_at: None,
            model: None,
            working_directory: "/home/user/myapp".to_string(),
            git_branch: Some("feat/api".to_string()),
            source_path: None,
            message_count: 1,
        };
        let session2 = Session {
            id: Uuid::new_v4(),
            tool: "aider".to_string(),
            tool_version: None,
            started_at: Utc::now(),
            ended_at: None,
            model: None,
            working_directory: "/home/user/myapp".to_string(),
            git_branch: Some("feat/api".to_string()),
            source_path: None,
            message_count: 1,
        };

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let msg1 =
            create_test_message(session1.id, 0, MessageRole::User, "API implementation work");
        let msg2 =
            create_test_message(session2.id, 0, MessageRole::User, "API implementation work");

        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Search with multiple filters
        let options = SearchOptions {
            query: "API".to_string(),
            limit: 10,
            tool: Some("claude-code".to_string()),
            branch: Some("api".to_string()),
            project: Some("myapp".to_string()),
            ..Default::default()
        };
        let results = db.search_with_options(&options).expect("search");

        // Results may include both message content match and metadata match from same session
        assert!(
            !results.is_empty(),
            "Should find at least 1 result matching all filters"
        );
        // All results should be from claude-code (the filtered tool)
        for result in &results {
            assert_eq!(
                result.tool, "claude-code",
                "All results should be from claude-code"
            );
        }
    }

    // ==================== Session Deletion Tests ====================

    #[test]
    fn test_delete_session_removes_all_data() {
        let (db, _dir) = create_test_db();

        let session = create_test_session("claude-code", "/project", Utc::now(), None);
        db.insert_session(&session).expect("insert session");

        // Add messages
        let msg1 = create_test_message(session.id, 0, MessageRole::User, "Hello");
        let msg2 = create_test_message(session.id, 1, MessageRole::Assistant, "Hi there");
        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Add a link
        let link = create_test_link(session.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("insert link");

        // Verify data exists
        assert_eq!(db.session_count().expect("count"), 1);
        assert_eq!(db.message_count().expect("count"), 2);
        assert_eq!(db.link_count().expect("count"), 1);

        // Delete the session
        let (msgs_deleted, links_deleted) = db.delete_session(&session.id).expect("delete");
        assert_eq!(msgs_deleted, 2, "Should delete 2 messages");
        assert_eq!(links_deleted, 1, "Should delete 1 link");

        // Verify all data is gone
        assert_eq!(db.session_count().expect("count"), 0);
        assert_eq!(db.message_count().expect("count"), 0);
        assert_eq!(db.link_count().expect("count"), 0);
        assert!(db.get_session(&session.id).expect("get").is_none());
    }

    #[test]
    fn test_delete_session_preserves_other_sessions() {
        let (db, _dir) = create_test_db();

        let session1 = create_test_session("claude-code", "/project1", Utc::now(), None);
        let session2 = create_test_session("aider", "/project2", Utc::now(), None);

        db.insert_session(&session1).expect("insert session1");
        db.insert_session(&session2).expect("insert session2");

        let msg1 = create_test_message(session1.id, 0, MessageRole::User, "Hello 1");
        let msg2 = create_test_message(session2.id, 0, MessageRole::User, "Hello 2");
        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Delete only session1
        db.delete_session(&session1.id).expect("delete");

        // Verify session2 still exists
        assert_eq!(db.session_count().expect("count"), 1);
        assert_eq!(db.message_count().expect("count"), 1);
        assert!(db.get_session(&session2.id).expect("get").is_some());
    }

    // ==================== Database Maintenance Tests ====================

    #[test]
    fn test_file_size() {
        let (db, _dir) = create_test_db();

        let size = db.file_size().expect("get size");
        assert!(size.is_some(), "Should have file size for file-based db");
        assert!(size.unwrap() > 0, "Database file should have size > 0");
    }

    #[test]
    fn test_vacuum() {
        let (db, _dir) = create_test_db();

        // Just verify vacuum runs without error
        db.vacuum().expect("vacuum should succeed");
    }

    #[test]
    fn test_count_sessions_older_than() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create sessions at different times
        let old_session =
            create_test_session("claude-code", "/project1", now - Duration::days(100), None);
        let recent_session =
            create_test_session("claude-code", "/project2", now - Duration::days(10), None);

        db.insert_session(&old_session).expect("insert old");
        db.insert_session(&recent_session).expect("insert recent");

        // Count sessions older than 30 days
        let cutoff = now - Duration::days(30);
        let count = db.count_sessions_older_than(cutoff).expect("count");
        assert_eq!(count, 1, "Should find 1 session older than 30 days");

        // Count sessions older than 200 days
        let old_cutoff = now - Duration::days(200);
        let old_count = db.count_sessions_older_than(old_cutoff).expect("count");
        assert_eq!(old_count, 0, "Should find 0 sessions older than 200 days");
    }

    #[test]
    fn test_delete_sessions_older_than() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Create sessions at different times
        let old_session =
            create_test_session("claude-code", "/project1", now - Duration::days(100), None);
        let recent_session =
            create_test_session("claude-code", "/project2", now - Duration::days(10), None);

        db.insert_session(&old_session).expect("insert old");
        db.insert_session(&recent_session).expect("insert recent");

        // Add messages to both
        let msg1 = create_test_message(old_session.id, 0, MessageRole::User, "Old message");
        let msg2 = create_test_message(recent_session.id, 0, MessageRole::User, "Recent message");
        db.insert_message(&msg1).expect("insert msg1");
        db.insert_message(&msg2).expect("insert msg2");

        // Delete sessions older than 30 days
        let cutoff = now - Duration::days(30);
        let deleted = db.delete_sessions_older_than(cutoff).expect("delete");
        assert_eq!(deleted, 1, "Should delete 1 session");

        // Verify only recent session remains
        assert_eq!(db.session_count().expect("count"), 1);
        assert!(db.get_session(&recent_session.id).expect("get").is_some());
        assert!(db.get_session(&old_session.id).expect("get").is_none());

        // Verify messages were also deleted
        assert_eq!(db.message_count().expect("count"), 1);
    }

    #[test]
    fn test_stats() {
        let (db, _dir) = create_test_db();
        let now = Utc::now();

        // Empty database stats
        let empty_stats = db.stats().expect("stats");
        assert_eq!(empty_stats.session_count, 0);
        assert_eq!(empty_stats.message_count, 0);
        assert_eq!(empty_stats.link_count, 0);
        assert!(empty_stats.oldest_session.is_none());
        assert!(empty_stats.newest_session.is_none());
        assert!(empty_stats.sessions_by_tool.is_empty());

        // Add some data
        let session1 =
            create_test_session("claude-code", "/project1", now - Duration::hours(2), None);
        let session2 = create_test_session("aider", "/project2", now - Duration::hours(1), None);
        let session3 = create_test_session("claude-code", "/project3", now, None);

        db.insert_session(&session1).expect("insert 1");
        db.insert_session(&session2).expect("insert 2");
        db.insert_session(&session3).expect("insert 3");

        let msg = create_test_message(session1.id, 0, MessageRole::User, "Hello");
        db.insert_message(&msg).expect("insert msg");

        let link = create_test_link(session1.id, Some("abc123"), LinkType::Commit);
        db.insert_link(&link).expect("insert link");

        // Check stats
        let stats = db.stats().expect("stats");
        assert_eq!(stats.session_count, 3);
        assert_eq!(stats.message_count, 1);
        assert_eq!(stats.link_count, 1);
        assert!(stats.oldest_session.is_some());
        assert!(stats.newest_session.is_some());

        // Check sessions by tool
        assert_eq!(stats.sessions_by_tool.len(), 2);
        // claude-code should come first (most sessions)
        assert_eq!(stats.sessions_by_tool[0].0, "claude-code");
        assert_eq!(stats.sessions_by_tool[0].1, 2);
        assert_eq!(stats.sessions_by_tool[1].0, "aider");
        assert_eq!(stats.sessions_by_tool[1].1, 1);
    }
}

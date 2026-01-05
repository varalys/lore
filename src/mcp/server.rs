//! MCP server implementation for Lore.
//!
//! Runs an MCP server on stdio transport, exposing Lore tools to
//! AI coding assistants like Claude Code.

use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::{
        CallToolResult, Content, ErrorCode, ErrorData as McpError, Implementation, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router,
    transport::stdio,
    ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::future::Future;

use crate::storage::models::{Message, SearchOptions, Session};
use crate::storage::Database;

// ============== Tool Parameter Types ==============

/// Parameters for the lore_search tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// The search query text.
    #[schemars(description = "Text to search for in session messages")]
    pub query: String,

    /// Maximum number of results to return.
    #[schemars(description = "Maximum number of results (default: 10)")]
    pub limit: Option<usize>,

    /// Filter by repository path prefix.
    #[schemars(description = "Filter by repository path prefix")]
    pub repo: Option<String>,

    /// Filter by AI tool name (e.g., claude-code, aider).
    #[schemars(description = "Filter by AI tool name (e.g., claude-code, aider)")]
    pub tool: Option<String>,

    /// Filter to sessions after this date (ISO 8601 or relative like 7d, 2w, 1m).
    #[schemars(description = "Filter to sessions after this date (ISO 8601 or 7d, 2w, 1m)")]
    pub since: Option<String>,
}

/// Parameters for the lore_get_session tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSessionParams {
    /// Session ID (full UUID or prefix).
    #[schemars(description = "Session ID (full UUID or short prefix like abc123)")]
    pub session_id: String,

    /// Whether to include full message content.
    #[schemars(description = "Include full message content (default: true)")]
    pub include_messages: Option<bool>,
}

/// Parameters for the lore_list_sessions tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSessionsParams {
    /// Maximum number of sessions to return.
    #[schemars(description = "Maximum number of sessions (default: 10)")]
    pub limit: Option<usize>,

    /// Filter by repository path prefix.
    #[schemars(description = "Filter by repository path prefix")]
    pub repo: Option<String>,
}

/// Parameters for the lore_get_context tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetContextParams {
    /// Repository path to get context for.
    #[schemars(description = "Repository path (defaults to current directory)")]
    pub repo: Option<String>,

    /// Whether to show detailed info for the most recent session only.
    #[schemars(description = "Show detailed info for the most recent session only")]
    pub last: Option<bool>,
}

/// Parameters for the lore_get_linked_sessions tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetLinkedSessionsParams {
    /// Git commit SHA (full or prefix).
    #[schemars(description = "Git commit SHA (full or short prefix)")]
    pub commit_sha: String,
}

// ============== Result Types ==============

/// A session in search results.
#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub id_short: String,
    pub tool: String,
    pub started_at: String,
    pub message_count: i32,
    pub working_directory: String,
    pub git_branch: Option<String>,
}

/// A search match result.
#[derive(Debug, Serialize)]
pub struct SearchMatch {
    pub session: SessionInfo,
    pub message_id: String,
    pub role: String,
    pub snippet: String,
    pub timestamp: String,
}

/// Search results response.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub total_matches: usize,
    pub matches: Vec<SearchMatch>,
}

/// A message for session transcript.
#[derive(Debug, Serialize)]
pub struct MessageInfo {
    pub index: i32,
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// Full session details response.
#[derive(Debug, Serialize)]
pub struct SessionDetailsResponse {
    pub session: SessionInfo,
    pub linked_commits: Vec<String>,
    pub messages: Option<Vec<MessageInfo>>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
}

/// Context response for a repository.
#[derive(Debug, Serialize)]
pub struct ContextResponse {
    pub working_directory: String,
    pub sessions: Vec<SessionInfo>,
    pub recent_messages: Option<Vec<MessageInfo>>,
}

/// Linked sessions response.
#[derive(Debug, Serialize)]
pub struct LinkedSessionsResponse {
    pub commit_sha: String,
    pub sessions: Vec<SessionInfo>,
}

// ============== Server Implementation ==============

/// The Lore MCP server.
///
/// Provides tools for querying Lore session data via MCP.
#[derive(Debug, Clone)]
pub struct LoreServer {
    tool_router: ToolRouter<LoreServer>,
}

impl LoreServer {
    /// Creates a new LoreServer.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for LoreServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates an McpError from an error message.
fn mcp_error(message: &str) -> McpError {
    McpError {
        code: ErrorCode(-32603),
        message: Cow::from(message.to_string()),
        data: None,
    }
}

#[tool_router]
impl LoreServer {
    /// Search Lore sessions by query text with optional filters.
    ///
    /// Searches message content using full-text search. Supports filtering
    /// by repository, tool, and date range.
    #[tool(description = "Search Lore session messages for text content")]
    async fn lore_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = search_impl(params);
        match result {
            Ok(response) => {
                let json = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing response: {e}"));
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Err(mcp_error(&format!("Search failed: {e}"))),
        }
    }

    /// Get full details of a Lore session by ID.
    ///
    /// Returns session metadata, linked commits, and optionally the full
    /// message transcript.
    #[tool(description = "Get full details of a Lore session by ID")]
    async fn lore_get_session(
        &self,
        Parameters(params): Parameters<GetSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = get_session_impl(params);
        match result {
            Ok(response) => {
                let json = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing response: {e}"));
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Err(mcp_error(&format!("Get session failed: {e}"))),
        }
    }

    /// List recent Lore sessions.
    ///
    /// Returns a list of recent sessions, optionally filtered by repository.
    #[tool(description = "List recent Lore sessions")]
    async fn lore_list_sessions(
        &self,
        Parameters(params): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = list_sessions_impl(params);
        match result {
            Ok(sessions) => {
                let json = serde_json::to_string_pretty(&sessions)
                    .unwrap_or_else(|e| format!("Error serializing response: {e}"));
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Err(mcp_error(&format!("List sessions failed: {e}"))),
        }
    }

    /// Get recent session context for a repository.
    ///
    /// Provides a summary of recent sessions for quick orientation.
    #[tool(description = "Get recent session context for a repository")]
    async fn lore_get_context(
        &self,
        Parameters(params): Parameters<GetContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = get_context_impl(params);
        match result {
            Ok(response) => {
                let json = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing response: {e}"));
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Err(mcp_error(&format!("Get context failed: {e}"))),
        }
    }

    /// Get sessions linked to a git commit.
    ///
    /// Returns all sessions that have been linked to the specified commit.
    #[tool(description = "Get Lore sessions linked to a git commit")]
    async fn lore_get_linked_sessions(
        &self,
        Parameters(params): Parameters<GetLinkedSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = get_linked_sessions_impl(params);
        match result {
            Ok(response) => {
                let json = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing response: {e}"));
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Err(mcp_error(&format!("Get linked sessions failed: {e}"))),
        }
    }
}

#[tool_handler]
impl ServerHandler for LoreServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Lore is a reasoning history system for code. It captures AI coding sessions \
                 and links them to git commits. Use these tools to search session history, \
                 view session transcripts, and find sessions linked to commits."
                    .to_string(),
            ),
        }
    }
}

// ============== Implementation Functions ==============

/// Parses a date string (ISO 8601 or relative like 7d, 2w, 1m) into a DateTime.
fn parse_date(date_str: &str) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    use chrono::{Duration, Utc};

    let date_str = date_str.trim().to_lowercase();

    // Try relative format first (e.g., "7d", "2w", "1m")
    if date_str.ends_with('d') {
        let days: i64 = date_str[..date_str.len() - 1].parse()?;
        return Ok(Utc::now() - Duration::days(days));
    }

    if date_str.ends_with('w') {
        let weeks: i64 = date_str[..date_str.len() - 1].parse()?;
        return Ok(Utc::now() - Duration::weeks(weeks));
    }

    if date_str.ends_with('m') {
        let months: i64 = date_str[..date_str.len() - 1].parse()?;
        return Ok(Utc::now() - Duration::days(months * 30));
    }

    // Try ISO 8601 format
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&date_str) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try date-only format
    if let Ok(date) = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
        let datetime = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid date"))?;
        return Ok(datetime.and_utc());
    }

    anyhow::bail!("Invalid date format: {date_str}")
}

/// Converts a Session to SessionInfo.
fn session_to_info(session: &Session) -> SessionInfo {
    SessionInfo {
        id: session.id.to_string(),
        id_short: session.id.to_string()[..8].to_string(),
        tool: session.tool.clone(),
        started_at: session.started_at.to_rfc3339(),
        message_count: session.message_count,
        working_directory: session.working_directory.clone(),
        git_branch: session.git_branch.clone(),
    }
}

/// Converts a Message to MessageInfo.
fn message_to_info(message: &Message) -> MessageInfo {
    MessageInfo {
        index: message.index,
        role: message.role.to_string(),
        content: message.content.text(),
        timestamp: message.timestamp.to_rfc3339(),
    }
}

/// Implementation of the search tool.
fn search_impl(params: SearchParams) -> anyhow::Result<SearchResponse> {
    let db = Database::open_default()?;

    // Build search index if needed
    if db.search_index_needs_rebuild()? {
        db.rebuild_search_index()?;
    }

    let since = params.since.as_ref().map(|s| parse_date(s)).transpose()?;

    let options = SearchOptions {
        query: params.query.clone(),
        limit: params.limit.unwrap_or(10),
        repo: params.repo,
        tool: params.tool,
        since,
        ..Default::default()
    };

    let results = db.search_with_options(&options)?;

    let matches: Vec<SearchMatch> = results
        .into_iter()
        .map(|r| SearchMatch {
            session: SessionInfo {
                id: r.session_id.to_string(),
                id_short: r.session_id.to_string()[..8].to_string(),
                tool: r.tool,
                started_at: r
                    .session_started_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
                message_count: r.session_message_count,
                working_directory: r.working_directory,
                git_branch: r.git_branch,
            },
            message_id: r.message_id.to_string(),
            role: r.role.to_string(),
            snippet: r.snippet,
            timestamp: r.timestamp.to_rfc3339(),
        })
        .collect();

    let total = matches.len();

    Ok(SearchResponse {
        query: params.query,
        total_matches: total,
        matches,
    })
}

/// Implementation of the get_session tool.
fn get_session_impl(params: GetSessionParams) -> anyhow::Result<SessionDetailsResponse> {
    let db = Database::open_default()?;

    // Try to find session by ID prefix
    let session_id = resolve_session_id(&db, &params.session_id)?;
    let session = db
        .get_session(&session_id)?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", params.session_id))?;

    // Get linked commits
    let links = db.get_links_by_session(&session_id)?;
    let linked_commits: Vec<String> = links.iter().filter_map(|l| l.commit_sha.clone()).collect();

    // Get messages if requested
    let messages = if params.include_messages.unwrap_or(true) {
        let msgs = db.get_messages(&session_id)?;
        Some(msgs.iter().map(message_to_info).collect())
    } else {
        None
    };

    // Get summary and tags
    let summary = db.get_summary(&session_id)?.map(|s| s.content);
    let tags: Vec<String> = db
        .get_tags(&session_id)?
        .into_iter()
        .map(|t| t.label)
        .collect();

    Ok(SessionDetailsResponse {
        session: session_to_info(&session),
        linked_commits,
        messages,
        summary,
        tags,
    })
}

/// Implementation of the list_sessions tool.
fn list_sessions_impl(params: ListSessionsParams) -> anyhow::Result<Vec<SessionInfo>> {
    let db = Database::open_default()?;

    let limit = params.limit.unwrap_or(10);
    let sessions = db.list_sessions(limit, params.repo.as_deref())?;

    Ok(sessions.iter().map(session_to_info).collect())
}

/// Implementation of the get_context tool.
fn get_context_impl(params: GetContextParams) -> anyhow::Result<ContextResponse> {
    let db = Database::open_default()?;

    let working_dir = params.repo.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    let limit = if params.last.unwrap_or(false) { 1 } else { 5 };
    let sessions = db.list_sessions(limit, Some(&working_dir))?;

    let session_infos: Vec<SessionInfo> = sessions.iter().map(session_to_info).collect();

    // Get recent messages for --last mode
    let recent_messages = if params.last.unwrap_or(false) && !sessions.is_empty() {
        let messages = db.get_messages(&sessions[0].id)?;
        let start = messages.len().saturating_sub(3);
        Some(messages[start..].iter().map(message_to_info).collect())
    } else {
        None
    };

    Ok(ContextResponse {
        working_directory: working_dir,
        sessions: session_infos,
        recent_messages,
    })
}

/// Implementation of the get_linked_sessions tool.
fn get_linked_sessions_impl(
    params: GetLinkedSessionsParams,
) -> anyhow::Result<LinkedSessionsResponse> {
    let db = Database::open_default()?;

    let links = db.get_links_by_commit(&params.commit_sha)?;

    let mut sessions = Vec::new();
    for link in links {
        if let Some(session) = db.get_session(&link.session_id)? {
            sessions.push(session_to_info(&session));
        }
    }

    Ok(LinkedSessionsResponse {
        commit_sha: params.commit_sha,
        sessions,
    })
}

/// Resolves a session ID prefix to a full UUID.
fn resolve_session_id(db: &Database, id_prefix: &str) -> anyhow::Result<uuid::Uuid> {
    // Try parsing as full UUID first
    if let Ok(uuid) = uuid::Uuid::parse_str(id_prefix) {
        return Ok(uuid);
    }

    // Otherwise, search by prefix
    let sessions = db.list_sessions(100, None)?;
    let matches: Vec<_> = sessions
        .iter()
        .filter(|s| s.id.to_string().starts_with(id_prefix))
        .collect();

    match matches.len() {
        0 => anyhow::bail!("No session found with ID prefix: {id_prefix}"),
        1 => Ok(matches[0].id),
        n => anyhow::bail!(
            "Ambiguous session ID prefix '{id_prefix}' matches {n} sessions. Use a longer prefix."
        ),
    }
}

/// Runs the MCP server on stdio transport.
///
/// This is a blocking call that processes MCP requests until the client
/// disconnects or an error occurs.
pub async fn run_server() -> Result<()> {
    let service = LoreServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_date_days() {
        let result = parse_date("7d").expect("Should parse 7d");
        let expected = chrono::Utc::now() - chrono::Duration::days(7);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_weeks() {
        let result = parse_date("2w").expect("Should parse 2w");
        let expected = chrono::Utc::now() - chrono::Duration::weeks(2);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_months() {
        let result = parse_date("1m").expect("Should parse 1m");
        let expected = chrono::Utc::now() - chrono::Duration::days(30);
        assert!((result - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_date_iso() {
        let result = parse_date("2024-01-15").expect("Should parse date");
        assert_eq!(result.format("%Y-%m-%d").to_string(), "2024-01-15");
    }

    #[test]
    fn test_parse_date_invalid() {
        assert!(parse_date("invalid").is_err());
        assert!(parse_date("abc123").is_err());
    }

    #[test]
    fn test_session_to_info() {
        use chrono::Utc;
        use uuid::Uuid;

        let session = Session {
            id: Uuid::new_v4(),
            tool: "claude-code".to_string(),
            tool_version: Some("2.0.0".to_string()),
            started_at: Utc::now(),
            ended_at: None,
            model: Some("claude-3-opus".to_string()),
            working_directory: "/home/user/project".to_string(),
            git_branch: Some("main".to_string()),
            source_path: None,
            message_count: 10,
            machine_id: None,
        };

        let info = session_to_info(&session);
        assert_eq!(info.tool, "claude-code");
        assert_eq!(info.message_count, 10);
        assert_eq!(info.working_directory, "/home/user/project");
        assert_eq!(info.git_branch, Some("main".to_string()));
        assert_eq!(info.id_short.len(), 8);
    }

    #[test]
    fn test_message_to_info() {
        use crate::storage::models::{MessageContent, MessageRole};
        use chrono::Utc;
        use uuid::Uuid;

        let message = Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::User,
            content: MessageContent::Text("Hello, world!".to_string()),
            model: None,
            git_branch: None,
            cwd: None,
        };

        let info = message_to_info(&message);
        assert_eq!(info.index, 0);
        assert_eq!(info.role, "user");
        assert_eq!(info.content, "Hello, world!");
    }
}

//! HTTP client for cloud API communication.
//!
//! Provides the `CloudClient` for interacting with the Lore cloud service,
//! including sync operations (push/pull) and status queries.

use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::{CloudError, DEFAULT_CLOUD_URL};

/// Cloud API client for sync operations.
pub struct CloudClient {
    /// HTTP client instance.
    client: Client,
    /// Base URL of the cloud service.
    base_url: String,
    /// API key for authentication (if logged in).
    api_key: Option<String>,
}

impl CloudClient {
    /// Creates a new cloud client with the default URL.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_CLOUD_URL.to_string(),
            api_key: None,
        }
    }

    /// Creates a new cloud client with a custom URL.
    pub fn with_url(base_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: None,
        }
    }

    /// Sets the API key for authentication.
    pub fn with_api_key(mut self, api_key: &str) -> Self {
        self.api_key = Some(api_key.to_string());
        self
    }

    /// Returns the configured base URL.
    #[allow(dead_code)]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Gets the sync status from the cloud service.
    ///
    /// Returns information about the user's sync state including session count,
    /// storage usage, and last sync time.
    pub fn status(&self) -> Result<SyncStatus, CloudError> {
        let api_key = self.api_key.as_ref().ok_or(CloudError::NotLoggedIn)?;

        let url = format!("{}/api/sync/status", self.base_url);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(CloudError::ServerError { status, message });
        }

        let body: ApiResponse<SyncStatus> = response.json()?;
        Ok(body.data)
    }

    /// Pushes sessions to the cloud service.
    ///
    /// Uploads encrypted session data to the cloud. Session metadata is stored
    /// unencrypted for display purposes, while message content is encrypted.
    pub fn push(&self, sessions: Vec<PushSession>) -> Result<PushResponse, CloudError> {
        let api_key = self.api_key.as_ref().ok_or(CloudError::NotLoggedIn)?;

        let url = format!("{}/api/sync/push", self.base_url);
        let payload = PushRequest { sessions };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(CloudError::ServerError { status, message });
        }

        let body: ApiResponse<PushResponse> = response.json()?;
        Ok(body.data)
    }

    /// Pulls sessions from the cloud service.
    ///
    /// Downloads sessions that have been modified since the given timestamp.
    /// Session message content is encrypted and must be decrypted by the caller.
    pub fn pull(&self, since: Option<DateTime<Utc>>) -> Result<PullResponse, CloudError> {
        let api_key = self.api_key.as_ref().ok_or(CloudError::NotLoggedIn)?;

        let mut url = format!("{}/api/sync/pull", self.base_url);
        if let Some(since) = since {
            url = format!("{}?since={}", url, since.to_rfc3339());
        }

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(CloudError::ServerError { status, message });
        }

        let body: ApiResponse<PullResponse> = response.json()?;
        Ok(body.data)
    }
}

impl Default for CloudClient {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== API Types ====================

/// Generic API response wrapper.
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    /// The response data.
    pub data: T,
}

/// Sync status response from the cloud service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// Number of sessions stored in the cloud.
    pub session_count: i64,

    /// Timestamp of the last sync operation.
    pub last_sync_at: Option<DateTime<Utc>>,

    /// Storage used in bytes.
    pub storage_used_bytes: i64,
}

/// Request payload for pushing sessions.
#[derive(Debug, Serialize)]
pub struct PushRequest {
    /// Sessions to push.
    pub sessions: Vec<PushSession>,
}

/// A session prepared for pushing to the cloud.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSession {
    /// Session UUID.
    pub id: String,

    /// Machine UUID that created this session.
    pub machine_id: String,

    /// Base64-encoded encrypted message data.
    pub encrypted_data: String,

    /// Unencrypted session metadata.
    pub metadata: SessionMetadata,

    /// When this session was last updated locally.
    pub updated_at: DateTime<Utc>,
}

/// Unencrypted session metadata for cloud display.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    /// Tool that created this session (e.g., "claude-code").
    pub tool_name: String,

    /// Working directory path.
    pub project_path: String,

    /// When the session started.
    pub started_at: DateTime<Utc>,

    /// When the session ended (if completed).
    pub ended_at: Option<DateTime<Utc>>,

    /// Number of messages in the session.
    pub message_count: i32,
}

/// Response from pushing sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
    /// Number of sessions successfully synced.
    pub synced_count: i64,

    /// Server timestamp for recording sync time.
    pub server_time: DateTime<Utc>,
}

/// Response from pulling sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResponse {
    /// Sessions to import.
    pub sessions: Vec<PullSession>,

    /// Server timestamp for recording sync time.
    pub server_time: DateTime<Utc>,
}

/// A session returned from the cloud for pulling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullSession {
    /// Session UUID.
    pub id: String,

    /// Machine UUID that created this session.
    pub machine_id: String,

    /// Base64-encoded encrypted message data.
    pub encrypted_data: String,

    /// Unencrypted session metadata.
    pub metadata: SessionMetadata,

    /// When this session was last updated on the server.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_client_new() {
        let client = CloudClient::new();
        assert_eq!(client.base_url(), DEFAULT_CLOUD_URL);
    }

    #[test]
    fn test_cloud_client_with_url() {
        let client = CloudClient::with_url("https://custom.example.com/");
        assert_eq!(client.base_url(), "https://custom.example.com");
    }

    #[test]
    fn test_cloud_client_with_url_no_trailing_slash() {
        let client = CloudClient::with_url("https://custom.example.com");
        assert_eq!(client.base_url(), "https://custom.example.com");
    }

    #[test]
    fn test_cloud_client_with_api_key() {
        let client = CloudClient::new().with_api_key("test_key");
        assert_eq!(client.api_key, Some("test_key".to_string()));
    }

    #[test]
    fn test_sync_status_deserialize() {
        let json = r#"{
            "sessionCount": 42,
            "lastSyncAt": "2024-01-01T00:00:00Z",
            "storageUsedBytes": 1234567
        }"#;

        let status: SyncStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.session_count, 42);
        assert!(status.last_sync_at.is_some());
        assert_eq!(status.storage_used_bytes, 1234567);
    }

    #[test]
    fn test_sync_status_deserialize_null_last_sync() {
        let json = r#"{
            "sessionCount": 0,
            "lastSyncAt": null,
            "storageUsedBytes": 0
        }"#;

        let status: SyncStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.session_count, 0);
        assert!(status.last_sync_at.is_none());
    }

    #[test]
    fn test_push_session_serialize() {
        let session = PushSession {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            machine_id: "machine-uuid".to_string(),
            encrypted_data: "base64encodeddata".to_string(),
            metadata: SessionMetadata {
                tool_name: "claude-code".to_string(),
                project_path: "/path/to/project".to_string(),
                started_at: Utc::now(),
                ended_at: None,
                message_count: 10,
            },
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("encryptedData"));
        assert!(json.contains("toolName"));
        assert!(json.contains("projectPath"));
    }

    #[test]
    fn test_session_metadata_serialize() {
        let metadata = SessionMetadata {
            tool_name: "aider".to_string(),
            project_path: "/home/user/project".to_string(),
            started_at: DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            ended_at: Some(
                DateTime::parse_from_rfc3339("2024-01-01T13:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            message_count: 25,
        };

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("\"toolName\":\"aider\""));
        assert!(json.contains("\"messageCount\":25"));
    }

    #[test]
    fn test_api_response_deserialize() {
        let json = r#"{
            "data": {
                "sessionCount": 5,
                "lastSyncAt": null,
                "storageUsedBytes": 1000
            }
        }"#;

        let response: ApiResponse<SyncStatus> = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.session_count, 5);
    }
}

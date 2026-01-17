//! Cloud sync module for Lore.
//!
//! Provides functionality for syncing sessions to the Lore cloud service,
//! including authentication, encryption, and API communication.
//!
//! # Submodules
//!
//! - `client` - HTTP client for cloud API communication
//! - `credentials` - Secure credential storage (keychain + fallback)
//! - `encryption` - End-to-end encryption for session content

pub mod client;
pub mod credentials;
pub mod encryption;

// Re-exports for external use
#[allow(unused_imports)]
pub use client::CloudClient;
#[allow(unused_imports)]
pub use credentials::{Credentials, CredentialsStore};
#[allow(unused_imports)]
pub use encryption::{decrypt_data, derive_key, encrypt_data};

/// Default cloud service URL.
pub const DEFAULT_CLOUD_URL: &str = "https://app.lore.varalys.com";

/// Service name for keyring storage.
pub const KEYRING_SERVICE: &str = "lore-cloud";

/// User identifier for API key in keyring.
pub const KEYRING_API_KEY_USER: &str = "api-key";

/// User identifier for encryption key in keyring.
pub const KEYRING_ENCRYPTION_KEY_USER: &str = "encryption-key";

/// Custom error type for cloud operations.
#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    /// Not logged in to the cloud service.
    #[error("Not logged in. Run 'lore login' first.")]
    NotLoggedIn,

    /// Authentication failed.
    #[error("Authentication failed: {0}")]
    #[allow(dead_code)]
    AuthFailed(String),

    /// Network or API error.
    #[error("Cloud API error: {0}")]
    #[allow(dead_code)]
    ApiError(String),

    /// HTTP request error.
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    /// Encryption or decryption error.
    #[error("Encryption error: {0}")]
    EncryptionError(String),

    /// Keyring storage error.
    #[error("Credential storage error: {0}")]
    KeyringError(String),

    /// Invalid or missing encryption key.
    #[error("Encryption key not set. Run 'lore cloud push' to set up encryption.")]
    #[allow(dead_code)]
    NoEncryptionKey,

    /// State mismatch during OAuth callback.
    #[error("OAuth state mismatch - possible CSRF attack")]
    #[allow(dead_code)]
    StateMismatch,

    /// Login timeout.
    #[error("Login timed out waiting for browser authentication")]
    #[allow(dead_code)]
    LoginTimeout,

    /// Server returned an error response.
    #[error("Server error ({status}): {message}")]
    ServerError { status: u16, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_error_display_not_logged_in() {
        let err = CloudError::NotLoggedIn;
        assert!(err.to_string().contains("Not logged in"));
    }

    #[test]
    fn test_cloud_error_display_auth_failed() {
        let err = CloudError::AuthFailed("invalid token".to_string());
        assert!(err.to_string().contains("invalid token"));
    }

    #[test]
    fn test_cloud_error_display_server_error() {
        let err = CloudError::ServerError {
            status: 500,
            message: "Internal error".to_string(),
        };
        assert!(err.to_string().contains("500"));
        assert!(err.to_string().contains("Internal error"));
    }

    #[test]
    fn test_default_cloud_url() {
        assert_eq!(DEFAULT_CLOUD_URL, "https://app.lore.varalys.com");
    }

    #[test]
    fn test_keyring_constants() {
        assert_eq!(KEYRING_SERVICE, "lore-cloud");
        assert_eq!(KEYRING_API_KEY_USER, "api-key");
        assert_eq!(KEYRING_ENCRYPTION_KEY_USER, "encryption-key");
    }
}

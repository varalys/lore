//! Credential storage for cloud authentication.
//!
//! Provides secure storage for API keys and encryption keys using the OS
//! keychain when available, with a file-based fallback for systems where
//! the keychain is not accessible.

use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use super::{CloudError, KEYRING_API_KEY_USER, KEYRING_ENCRYPTION_KEY_USER, KEYRING_SERVICE};

/// Cloud service credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// The API key for authenticating with the cloud service.
    pub api_key: String,

    /// User email address associated with the account.
    pub email: String,

    /// Subscription plan (e.g., "free", "pro").
    pub plan: String,

    /// Cloud service URL (for custom deployments).
    #[serde(default = "default_cloud_url")]
    pub cloud_url: String,
}

fn default_cloud_url() -> String {
    super::DEFAULT_CLOUD_URL.to_string()
}

/// Credential storage abstraction.
///
/// Uses the OS keychain for secure storage when available, with a fallback
/// to a JSON file in the Lore config directory.
pub struct CredentialsStore {
    /// Whether keyring is available on this system.
    keyring_available: bool,
}

impl CredentialsStore {
    /// Creates a new credential store.
    ///
    /// Automatically detects whether the OS keychain is available.
    pub fn new() -> Self {
        let keyring_available = Self::test_keyring_available();
        Self { keyring_available }
    }

    /// Tests whether the keyring is available by attempting a dummy operation.
    fn test_keyring_available() -> bool {
        // Try to create an entry - this will fail on systems without keyring support
        match Entry::new(KEYRING_SERVICE, "test-availability") {
            Ok(entry) => {
                // Try to get a non-existent key - should return NotFound, not an error
                match entry.get_password() {
                    Ok(_) => true,
                    Err(keyring::Error::NoEntry) => true,
                    Err(_) => false,
                }
            }
            Err(_) => false,
        }
    }

    /// Stores credentials securely.
    ///
    /// Attempts to use the OS keychain first, falling back to file storage.
    pub fn store(&self, credentials: &Credentials) -> Result<(), CloudError> {
        if self.keyring_available {
            self.store_to_keyring(credentials)
        } else {
            self.store_to_file(credentials)
        }
    }

    /// Loads stored credentials.
    ///
    /// Checks the keychain first, then falls back to file storage.
    pub fn load(&self) -> Result<Option<Credentials>, CloudError> {
        // Try keyring first
        if self.keyring_available {
            if let Some(creds) = self.load_from_keyring()? {
                return Ok(Some(creds));
            }
        }

        // Fall back to file
        self.load_from_file()
    }

    /// Deletes stored credentials.
    ///
    /// Removes credentials from both keyring and file storage.
    pub fn delete(&self) -> Result<(), CloudError> {
        // Delete from keyring if available
        if self.keyring_available {
            self.delete_from_keyring()?;
        }

        // Also delete from file
        self.delete_from_file()?;

        Ok(())
    }

    /// Stores the derived encryption key securely.
    ///
    /// The encryption key is stored separately from credentials and should
    /// be a hex-encoded string of the derived key bytes.
    pub fn store_encryption_key(&self, key_hex: &str) -> Result<(), CloudError> {
        if self.keyring_available {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            entry
                .set_password(key_hex)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
        } else {
            // Fall back to file storage
            let path = Self::encryption_key_path()?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| CloudError::KeyringError(format!("Failed to create dir: {e}")))?;
            }
            fs::write(&path, key_hex)
                .map_err(|e| CloudError::KeyringError(format!("Failed to write key: {e}")))?;
        }
        Ok(())
    }

    /// Loads the stored encryption key.
    ///
    /// Returns the hex-encoded encryption key, or None if not stored.
    pub fn load_encryption_key(&self) -> Result<Option<String>, CloudError> {
        if self.keyring_available {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            match entry.get_password() {
                Ok(key) => return Ok(Some(key)),
                Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(CloudError::KeyringError(e.to_string())),
            }
        }

        // Fall back to file
        let path = Self::encryption_key_path()?;
        if path.exists() {
            let key = fs::read_to_string(&path)
                .map_err(|e| CloudError::KeyringError(format!("Failed to read key: {e}")))?;
            return Ok(Some(key.trim().to_string()));
        }

        Ok(None)
    }

    /// Deletes the stored encryption key.
    pub fn delete_encryption_key(&self) -> Result<(), CloudError> {
        if self.keyring_available {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            match entry.delete_credential() {
                Ok(()) => {}
                Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(CloudError::KeyringError(e.to_string())),
            }
        }

        // Also delete from file
        let path = Self::encryption_key_path()?;
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| CloudError::KeyringError(format!("Failed to delete key file: {e}")))?;
        }

        Ok(())
    }

    // ==================== Keyring operations ====================

    fn store_to_keyring(&self, credentials: &Credentials) -> Result<(), CloudError> {
        let entry = Entry::new(KEYRING_SERVICE, KEYRING_API_KEY_USER)
            .map_err(|e| CloudError::KeyringError(e.to_string()))?;

        // Store credentials as JSON
        let json = serde_json::to_string(credentials)
            .map_err(|e| CloudError::KeyringError(format!("Serialization error: {e}")))?;

        entry
            .set_password(&json)
            .map_err(|e| CloudError::KeyringError(e.to_string()))?;

        Ok(())
    }

    fn load_from_keyring(&self) -> Result<Option<Credentials>, CloudError> {
        let entry = Entry::new(KEYRING_SERVICE, KEYRING_API_KEY_USER)
            .map_err(|e| CloudError::KeyringError(e.to_string()))?;

        match entry.get_password() {
            Ok(json) => {
                let credentials: Credentials = serde_json::from_str(&json)
                    .map_err(|e| CloudError::KeyringError(format!("Deserialization error: {e}")))?;
                Ok(Some(credentials))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CloudError::KeyringError(e.to_string())),
        }
    }

    fn delete_from_keyring(&self) -> Result<(), CloudError> {
        let entry = Entry::new(KEYRING_SERVICE, KEYRING_API_KEY_USER)
            .map_err(|e| CloudError::KeyringError(e.to_string()))?;

        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already deleted
            Err(e) => Err(CloudError::KeyringError(e.to_string())),
        }
    }

    // ==================== File operations ====================

    fn credentials_path() -> Result<PathBuf, CloudError> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| CloudError::KeyringError("Could not find home directory".to_string()))?
            .join(".lore");

        Ok(config_dir.join("credentials.json"))
    }

    fn encryption_key_path() -> Result<PathBuf, CloudError> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| CloudError::KeyringError("Could not find home directory".to_string()))?
            .join(".lore");

        Ok(config_dir.join("encryption.key"))
    }

    fn store_to_file(&self, credentials: &Credentials) -> Result<(), CloudError> {
        let path = Self::credentials_path()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CloudError::KeyringError(format!("Failed to create config directory: {e}"))
            })?;
        }

        let json = serde_json::to_string_pretty(credentials)
            .map_err(|e| CloudError::KeyringError(format!("Serialization error: {e}")))?;

        fs::write(&path, json).map_err(|e| {
            CloudError::KeyringError(format!("Failed to write credentials file: {e}"))
        })?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, perms).map_err(|e| {
                CloudError::KeyringError(format!("Failed to set file permissions: {e}"))
            })?;
        }

        Ok(())
    }

    fn load_from_file(&self) -> Result<Option<Credentials>, CloudError> {
        let path = Self::credentials_path()?;

        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&path).map_err(|e| {
            CloudError::KeyringError(format!("Failed to read credentials file: {e}"))
        })?;

        let credentials: Credentials = serde_json::from_str(&json)
            .map_err(|e| CloudError::KeyringError(format!("Invalid credentials file: {e}")))?;

        Ok(Some(credentials))
    }

    fn delete_from_file(&self) -> Result<(), CloudError> {
        let path = Self::credentials_path()?;

        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                CloudError::KeyringError(format!("Failed to delete credentials file: {e}"))
            })?;
        }

        Ok(())
    }
}

impl Default for CredentialsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Checks if the user is currently logged in.
///
/// Returns true if valid credentials are stored, false otherwise.
#[allow(dead_code)]
pub fn is_logged_in() -> bool {
    let store = CredentialsStore::new();
    matches!(store.load(), Ok(Some(_)))
}

/// Gets the current credentials if logged in.
///
/// Returns None if not logged in or credentials cannot be loaded.
#[allow(dead_code)]
pub fn get_credentials() -> Option<Credentials> {
    let store = CredentialsStore::new();
    store.load().ok().flatten()
}

/// Requires login, returning an error if not logged in.
///
/// This is a convenience function for commands that require authentication.
pub fn require_login() -> Result<Credentials> {
    let store = CredentialsStore::new();
    store
        .load()
        .context("Failed to check login status")?
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'lore login' first."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_default_cloud_url() {
        let creds = Credentials {
            api_key: "test".to_string(),
            email: "test@example.com".to_string(),
            plan: "free".to_string(),
            cloud_url: default_cloud_url(),
        };
        assert_eq!(creds.cloud_url, super::super::DEFAULT_CLOUD_URL);
    }

    #[test]
    fn test_credentials_serialization() {
        let creds = Credentials {
            api_key: "lore_test123".to_string(),
            email: "user@example.com".to_string(),
            plan: "pro".to_string(),
            cloud_url: "https://custom.example.com".to_string(),
        };

        let json = serde_json::to_string(&creds).unwrap();
        let parsed: Credentials = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.api_key, creds.api_key);
        assert_eq!(parsed.email, creds.email);
        assert_eq!(parsed.plan, creds.plan);
        assert_eq!(parsed.cloud_url, creds.cloud_url);
    }

    #[test]
    fn test_credentials_deserialization_default_url() {
        // Test that cloud_url gets a default value when not present in JSON
        let json = r#"{"api_key":"test","email":"test@example.com","plan":"free"}"#;
        let creds: Credentials = serde_json::from_str(json).unwrap();
        assert_eq!(creds.cloud_url, super::super::DEFAULT_CLOUD_URL);
    }

    #[test]
    fn test_is_logged_in_returns_bool() {
        // This test verifies the function exists and runs without panic.
        // The actual result depends on whether there are existing
        // credentials on the system.
        let _result: bool = is_logged_in();
    }
}

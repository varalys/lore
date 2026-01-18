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
/// By default, stores credentials in a JSON file (~/.lore/credentials.json).
/// Can optionally use the OS keychain (macOS Keychain, GNOME Keyring, Windows
/// Credential Manager) when enabled via `use_keychain` config option.
pub struct CredentialsStore {
    /// Whether to use keyring (enabled via config and available on system).
    use_keyring: bool,
    /// Base directory for file-backed storage (defaults to ~/.lore).
    base_dir: Option<PathBuf>,
}

impl CredentialsStore {
    /// Creates a new credential store with file-based storage (default).
    ///
    /// Credentials are stored in ~/.lore/credentials.json with restricted permissions.
    pub fn new() -> Self {
        Self {
            use_keyring: false,
            base_dir: None,
        }
    }

    /// Creates a credential store with optional keychain support.
    ///
    /// If `use_keychain` is true and the OS keychain is available, credentials
    /// will be stored in the keychain. Otherwise, falls back to file storage.
    ///
    /// Note: On first keychain access, the OS may prompt for permission.
    pub fn with_keychain(use_keychain: bool) -> Self {
        let use_keyring = if use_keychain {
            Self::is_keyring_available()
        } else {
            false
        };
        Self {
            use_keyring,
            base_dir: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_base_dir(base_dir: PathBuf, use_keychain: bool) -> Self {
        let use_keyring = if use_keychain {
            Self::is_keyring_available()
        } else {
            false
        };
        Self {
            use_keyring,
            base_dir: Some(base_dir),
        }
    }

    /// Tests whether the keyring is available by attempting a dummy operation.
    ///
    /// This is useful for checking if the OS keychain can be used before
    /// prompting the user about credential storage options.
    pub fn is_keyring_available() -> bool {
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

    /// Checks if a secret service is likely available on Linux.
    ///
    /// On Linux, the keyring crate requires a running secret service
    /// (gnome-keyring, kwallet, etc.) to function. This method checks
    /// for common indicators that a secret service is available.
    ///
    /// On non-Linux platforms, this always returns true since they have
    /// built-in credential storage (macOS Keychain, Windows Credential Manager).
    #[cfg(target_os = "linux")]
    pub fn is_secret_service_available() -> bool {
        // Check for common secret service environment indicators
        // DBUS_SESSION_BUS_ADDRESS is required for secret service communication
        if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_err() {
            return false;
        }

        // Try to actually test the keyring - this is the most reliable check
        Self::is_keyring_available()
    }

    /// On non-Linux platforms, secret service is always available.
    #[cfg(not(target_os = "linux"))]
    pub fn is_secret_service_available() -> bool {
        true
    }

    /// Stores credentials securely.
    ///
    /// Uses file storage by default, or keychain if enabled and available.
    pub fn store(&self, credentials: &Credentials) -> Result<(), CloudError> {
        if self.use_keyring {
            self.store_to_keyring(credentials)
        } else {
            self.store_to_file(credentials)
        }
    }

    /// Loads stored credentials.
    ///
    /// Loads from keychain if enabled, otherwise from file storage.
    /// Also checks the alternate location for migration purposes.
    pub fn load(&self) -> Result<Option<Credentials>, CloudError> {
        if self.use_keyring {
            // Try keyring first, fall back to file
            if let Some(creds) = self.load_from_keyring()? {
                return Ok(Some(creds));
            }
            self.load_from_file()
        } else {
            // Try file first, fall back to keyring (for migration)
            if let Some(creds) = self.load_from_file()? {
                return Ok(Some(creds));
            }
            // Check keyring as fallback (user may have stored there previously)
            if Self::is_keyring_available() {
                self.load_from_keyring()
            } else {
                Ok(None)
            }
        }
    }

    /// Deletes stored credentials.
    ///
    /// Removes credentials from both file and keyring storage to ensure
    /// complete cleanup regardless of how they were stored.
    pub fn delete(&self) -> Result<(), CloudError> {
        // Delete from file
        self.delete_from_file()?;

        // Also delete from keyring if available (cleanup any legacy storage)
        if Self::is_keyring_available() {
            self.delete_from_keyring()?;
        }

        Ok(())
    }

    /// Stores the derived encryption key securely.
    ///
    /// The encryption key is stored separately from credentials and should
    /// be a hex-encoded string of the derived key bytes.
    pub fn store_encryption_key(&self, key_hex: &str) -> Result<(), CloudError> {
        if self.use_keyring {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            entry
                .set_password(key_hex)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
        } else {
            // Use file storage
            let path = self.encryption_key_path()?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| CloudError::KeyringError(format!("Failed to create dir: {e}")))?;
            }
            fs::write(&path, key_hex)
                .map_err(|e| CloudError::KeyringError(format!("Failed to write key: {e}")))?;

            // Set restrictive permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o600);
                fs::set_permissions(&path, perms).map_err(|e| {
                    CloudError::KeyringError(format!("Failed to set permissions: {e}"))
                })?;
            }
        }
        Ok(())
    }

    /// Loads the stored encryption key.
    ///
    /// Returns the hex-encoded encryption key, or None if not stored.
    pub fn load_encryption_key(&self) -> Result<Option<String>, CloudError> {
        if self.use_keyring {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            match entry.get_password() {
                Ok(key) => return Ok(Some(key)),
                Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(CloudError::KeyringError(e.to_string())),
            }
        }

        // Check file storage
        let path = self.encryption_key_path()?;
        if path.exists() {
            let key = fs::read_to_string(&path)
                .map_err(|e| CloudError::KeyringError(format!("Failed to read key: {e}")))?;
            return Ok(Some(key.trim().to_string()));
        }

        // Check keyring as fallback (for migration from keyring to file)
        if !self.use_keyring && Self::is_keyring_available() {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            match entry.get_password() {
                Ok(key) => return Ok(Some(key)),
                Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(CloudError::KeyringError(e.to_string())),
            }
        }

        Ok(None)
    }

    /// Deletes the stored encryption key.
    ///
    /// Removes from both file and keyring to ensure complete cleanup.
    pub fn delete_encryption_key(&self) -> Result<(), CloudError> {
        // Delete from file
        let path = self.encryption_key_path()?;
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| CloudError::KeyringError(format!("Failed to delete key file: {e}")))?;
        }

        // Also delete from keyring if available (cleanup any legacy storage)
        if Self::is_keyring_available() {
            let entry = Entry::new(KEYRING_SERVICE, KEYRING_ENCRYPTION_KEY_USER)
                .map_err(|e| CloudError::KeyringError(e.to_string()))?;
            match entry.delete_credential() {
                Ok(()) => {}
                Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(CloudError::KeyringError(e.to_string())),
            }
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

    fn credentials_path(&self) -> Result<PathBuf, CloudError> {
        let config_dir = match &self.base_dir {
            Some(base_dir) => base_dir.clone(),
            None => dirs::home_dir()
                .ok_or_else(|| {
                    CloudError::KeyringError("Could not find home directory".to_string())
                })?
                .join(".lore"),
        };

        Ok(config_dir.join("credentials.json"))
    }

    fn encryption_key_path(&self) -> Result<PathBuf, CloudError> {
        let config_dir = match &self.base_dir {
            Some(base_dir) => base_dir.clone(),
            None => dirs::home_dir()
                .ok_or_else(|| {
                    CloudError::KeyringError("Could not find home directory".to_string())
                })?
                .join(".lore"),
        };

        Ok(config_dir.join("encryption.key"))
    }

    fn store_to_file(&self, credentials: &Credentials) -> Result<(), CloudError> {
        let path = self.credentials_path()?;

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
        let path = self.credentials_path()?;

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
        let path = self.credentials_path()?;

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
/// Respects the `use_keychain` config setting.
#[allow(dead_code)]
pub fn is_logged_in() -> bool {
    let use_keychain = crate::config::Config::load()
        .map(|c| c.use_keychain)
        .unwrap_or(false);
    let store = CredentialsStore::with_keychain(use_keychain);
    matches!(store.load(), Ok(Some(_)))
}

/// Gets the current credentials if logged in.
///
/// Returns None if not logged in or credentials cannot be loaded.
/// Respects the `use_keychain` config setting.
#[allow(dead_code)]
pub fn get_credentials() -> Option<Credentials> {
    let use_keychain = crate::config::Config::load()
        .map(|c| c.use_keychain)
        .unwrap_or(false);
    let store = CredentialsStore::with_keychain(use_keychain);
    get_credentials_with_store(&store)
}

/// Requires login, returning an error if not logged in.
///
/// This is a convenience function for commands that require authentication.
/// Respects the `use_keychain` config setting.
pub fn require_login() -> Result<Credentials> {
    let use_keychain = crate::config::Config::load()
        .map(|c| c.use_keychain)
        .unwrap_or(false);
    let store = CredentialsStore::with_keychain(use_keychain);
    require_login_with_store(&store)
}

fn get_credentials_with_store(store: &CredentialsStore) -> Option<Credentials> {
    store.load().ok().flatten()
}

fn require_login_with_store(store: &CredentialsStore) -> Result<Credentials> {
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

    #[test]
    fn test_is_keyring_available_smoke() {
        // This test verifies the function exists and returns a boolean.
        // The actual result depends on the system's keychain support.
        let _result: bool = CredentialsStore::is_keyring_available();
    }

    #[test]
    fn test_is_secret_service_available_smoke() {
        // This test verifies the function exists and returns a boolean.
        // On macOS and Windows, this should always return true.
        // On Linux, it depends on whether a secret service is running.
        let _result: bool = CredentialsStore::is_secret_service_available();
    }

    #[test]
    fn test_require_login_with_store_deterministic() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = CredentialsStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let creds = Credentials {
            api_key: "test_key".to_string(),
            email: "user@example.com".to_string(),
            plan: "pro".to_string(),
            cloud_url: default_cloud_url(),
        };

        store.store(&creds).unwrap();
        let loaded = require_login_with_store(&store).unwrap();
        assert_eq!(loaded.email, creds.email);
        assert_eq!(loaded.api_key, creds.api_key);
    }

    #[test]
    fn test_get_credentials_with_store_deterministic() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = CredentialsStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let creds = Credentials {
            api_key: "test_key".to_string(),
            email: "user@example.com".to_string(),
            plan: "free".to_string(),
            cloud_url: default_cloud_url(),
        };

        store.store(&creds).unwrap();
        let loaded = get_credentials_with_store(&store).unwrap();
        assert_eq!(loaded.email, creds.email);
        assert_eq!(loaded.api_key, creds.api_key);
    }
}

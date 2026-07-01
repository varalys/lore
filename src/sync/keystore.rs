//! Passphrase-based key management for git-ref sync.
//!
//! The no-account model derives the encryption key from a passphrase plus a
//! salt. The canonical salt for a store lives in the ref tree at `meta/salt`
//! (plaintext; a salt is not secret), so every machine that shares the
//! passphrase derives the same key. This module consumes a salt provided by the
//! caller (read from the ref by the [`gitref`](super::gitref) layer) and
//! produces a fresh salt when a store is first initialized.
//!
//! Unlike the legacy cloud module, sync keys do NOT share the single
//! `encryption.key` file / `encryption-key` keychain entry. Each repo or store
//! has its own passphrase plus salt and therefore its own derived key, so the
//! storage is namespaced by a store identifier derived from the salt. Reusing
//! the cloud slot would both corrupt the live cloud key and make it impossible
//! to hold more than one store key at a time. The file/keychain abstraction here
//! mirrors the pattern in [`crate::cloud::credentials`] but is kept self
//! contained and never touches the cloud credentials.

use keyring::Entry;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

use super::encryption::{decode_key_hex, derive_key, encode_key_hex, generate_salt};
use super::SyncError;

/// Keychain service name for sync store keys.
///
/// Deliberately distinct from the cloud module's `lore-cloud` service so the two
/// never collide. Each store's key is stored under this service with the
/// store-id as the account/user.
const SYNC_KEYRING_SERVICE: &str = "lore-sync";

/// Derives the store encryption key from a passphrase and a salt.
///
/// The salt is supplied by the caller (read from the store's `meta/salt` blob),
/// keeping salt I/O in the git layer and key math here. The same passphrase and
/// salt always yield the same 32-byte key.
pub fn derive_store_key(passphrase: &str, salt: &[u8]) -> Result<Vec<u8>, SyncError> {
    derive_key(passphrase, salt)
}

/// Generates a fresh random salt for a newly initialized store.
///
/// The caller writes the returned bytes to the store's `meta/salt` blob so
/// other machines can derive the same key.
pub fn generate_store_salt() -> Vec<u8> {
    generate_salt()
}

/// Derives a stable store identifier from a store's salt.
///
/// The identifier is the hex-encoded SHA-256 of the salt bytes. Because every
/// repo or store has its own salt, the resulting id is distinct per store, which
/// is what lets each store occupy its own key slot (file name or keychain
/// account) without clobbering another store's key.
pub fn store_id_from_salt(salt: &[u8]) -> String {
    let digest = Sha256::digest(salt);
    encode_key_hex(&digest)
}

/// Persists the derived encryption key for a lore store, keyed by store-id.
///
/// Keys live in a dedicated namespace so the cloud encryption key and any other
/// store's key are never touched. Depending on `use_keychain`, the key lands in
/// the OS keychain (service `lore-sync`, account = store-id) or a
/// permission-restricted file at `~/.lore/sync-keys/<store-id>.key`.
pub struct KeyStore {
    /// Whether to use the OS keychain (config-driven and available on system).
    use_keyring: bool,
    /// Base directory for file-backed storage (defaults to `~/.lore`).
    base_dir: Option<PathBuf>,
}

impl KeyStore {
    /// Creates a key store backed by file storage (the default).
    pub fn new() -> Self {
        Self {
            use_keyring: false,
            base_dir: None,
        }
    }

    /// Creates a key store, using the OS keychain when requested and available.
    ///
    /// Falls back to file storage when the keychain is unavailable, matching the
    /// behavior of the cloud credentials store.
    pub fn with_keychain(use_keychain: bool) -> Self {
        Self {
            use_keyring: use_keychain && Self::is_keyring_available(),
            base_dir: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_base_dir(base_dir: std::path::PathBuf, use_keychain: bool) -> Self {
        Self {
            use_keyring: use_keychain && Self::is_keyring_available(),
            base_dir: Some(base_dir),
        }
    }

    /// Tests whether the OS keychain is usable for sync keys.
    fn is_keyring_available() -> bool {
        match Entry::new(SYNC_KEYRING_SERVICE, "test-availability") {
            Ok(entry) => matches!(entry.get_password(), Ok(_) | Err(keyring::Error::NoEntry)),
            Err(_) => false,
        }
    }

    /// Stores the derived key bytes for a store (hex-encoded under the hood).
    ///
    /// `store_id` namespaces the slot; obtain it from [`store_id_from_salt`].
    pub fn store_key(&self, store_id: &str, key: &[u8]) -> Result<(), SyncError> {
        let key_hex = encode_key_hex(key);
        if self.use_keyring {
            let entry = self.keyring_entry(store_id)?;
            entry
                .set_password(&key_hex)
                .map_err(|e| SyncError::KeyStorage(e.to_string()))?;
        } else {
            self.store_to_file(store_id, &key_hex)?;
        }
        Ok(())
    }

    /// Loads the stored key bytes for a store, or `None` if no key is stored.
    pub fn load_key(&self, store_id: &str) -> Result<Option<Vec<u8>>, SyncError> {
        if self.use_keyring {
            let entry = self.keyring_entry(store_id)?;
            match entry.get_password() {
                Ok(key_hex) => return Ok(Some(decode_key_hex(&key_hex)?)),
                Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(SyncError::KeyStorage(e.to_string())),
            }
        }

        let path = self.key_path(store_id)?;
        if path.exists() {
            let key_hex = fs::read_to_string(&path)
                .map_err(|e| SyncError::KeyStorage(format!("Failed to read key: {e}")))?;
            return Ok(Some(decode_key_hex(key_hex.trim())?));
        }

        Ok(None)
    }

    /// Deletes any stored key for a store from both file and keychain storage.
    ///
    /// Part of the key-store foundation's public API. Retained for a future
    /// store reset/forget flow; the per-repo `lore sync` command only stores and
    /// loads keys, so the binary does not yet call this directly.
    #[allow(dead_code)]
    pub fn delete_key(&self, store_id: &str) -> Result<(), SyncError> {
        let path = self.key_path(store_id)?;
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| SyncError::KeyStorage(format!("Failed to delete key file: {e}")))?;
        }

        if Self::is_keyring_available() {
            let entry = self.keyring_entry(store_id)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(SyncError::KeyStorage(e.to_string())),
            }
        }

        Ok(())
    }

    /// Builds a keychain entry for a store under the dedicated sync service.
    fn keyring_entry(&self, store_id: &str) -> Result<Entry, SyncError> {
        Entry::new(SYNC_KEYRING_SERVICE, store_id).map_err(|e| SyncError::KeyStorage(e.to_string()))
    }

    /// Returns the `sync-keys` directory used for file-backed key storage.
    fn sync_keys_dir(&self) -> Result<PathBuf, SyncError> {
        let base = match &self.base_dir {
            Some(base_dir) => base_dir.clone(),
            None => dirs::home_dir()
                .ok_or_else(|| SyncError::KeyStorage("Could not find home directory".to_string()))?
                .join(".lore"),
        };
        Ok(base.join("sync-keys"))
    }

    /// Returns the file path for a store's key.
    fn key_path(&self, store_id: &str) -> Result<PathBuf, SyncError> {
        Ok(self.sync_keys_dir()?.join(format!("{store_id}.key")))
    }

    /// Writes a hex-encoded key to the per-store file with locked-down perms.
    ///
    /// The `sync-keys` directory is created 0700 and the key file 0600 so other
    /// users on the machine cannot read the stored key material.
    fn store_to_file(&self, store_id: &str, key_hex: &str) -> Result<(), SyncError> {
        let dir = self.sync_keys_dir()?;
        fs::create_dir_all(&dir)
            .map_err(|e| SyncError::KeyStorage(format!("Failed to create key dir: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
                SyncError::KeyStorage(format!("Failed to set key dir permissions: {e}"))
            })?;
        }

        let path = dir.join(format!("{store_id}.key"));
        fs::write(&path, key_hex)
            .map_err(|e| SyncError::KeyStorage(format!("Failed to write key: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| {
                SyncError::KeyStorage(format!("Failed to set key permissions: {e}"))
            })?;
        }

        Ok(())
    }
}

impl Default for KeyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::encryption::KEY_SIZE;

    #[test]
    fn test_derive_store_key_deterministic() {
        let salt = generate_store_salt();
        let key1 = derive_store_key("my passphrase", &salt).unwrap();
        let key2 = derive_store_key("my passphrase", &salt).unwrap();

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), KEY_SIZE);
    }

    #[test]
    fn test_derive_store_key_differs_by_passphrase() {
        let salt = generate_store_salt();
        let key1 = derive_store_key("passphrase one", &salt).unwrap();
        let key2 = derive_store_key("passphrase two", &salt).unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_derive_store_key_differs_by_salt() {
        let salt1 = generate_store_salt();
        let salt2 = generate_store_salt();
        let key1 = derive_store_key("same passphrase", &salt1).unwrap();
        let key2 = derive_store_key("same passphrase", &salt2).unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_generate_store_salt_is_random() {
        let salt1 = generate_store_salt();
        let salt2 = generate_store_salt();
        assert_ne!(salt1, salt2);
    }

    #[test]
    fn test_store_id_from_salt_deterministic_and_distinct() {
        let salt1 = generate_store_salt();
        let salt2 = generate_store_salt();

        // Same salt always yields the same id.
        assert_eq!(store_id_from_salt(&salt1), store_id_from_salt(&salt1));
        // Different salts yield different ids.
        assert_ne!(store_id_from_salt(&salt1), store_id_from_salt(&salt2));
        // The id is a hex SHA-256 (64 hex chars).
        assert_eq!(store_id_from_salt(&salt1).len(), 64);
    }

    #[test]
    fn test_key_store_round_trip() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = KeyStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let salt = generate_store_salt();
        let store_id = store_id_from_salt(&salt);
        let key = derive_store_key("secret passphrase", &salt).unwrap();

        store.store_key(&store_id, &key).unwrap();
        let loaded = store.load_key(&store_id).unwrap();

        assert_eq!(loaded, Some(key));
    }

    #[test]
    fn test_key_store_isolated_per_store_id() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = KeyStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let salt_a = generate_store_salt();
        let salt_b = generate_store_salt();
        let id_a = store_id_from_salt(&salt_a);
        let id_b = store_id_from_salt(&salt_b);

        let key_a = derive_store_key("passphrase a", &salt_a).unwrap();
        let key_b = derive_store_key("passphrase b", &salt_b).unwrap();

        store.store_key(&id_a, &key_a).unwrap();
        store.store_key(&id_b, &key_b).unwrap();

        // Each store-id loads its own key, not the other's.
        assert_eq!(store.load_key(&id_a).unwrap(), Some(key_a.clone()));
        assert_eq!(store.load_key(&id_b).unwrap(), Some(key_b));
        assert_ne!(
            store.load_key(&id_a).unwrap(),
            store.load_key(&id_b).unwrap()
        );

        // Deleting one store's key leaves the other intact.
        store.delete_key(&id_a).unwrap();
        assert!(store.load_key(&id_a).unwrap().is_none());
        assert!(store.load_key(&id_b).unwrap().is_some());
    }

    #[test]
    fn test_key_store_load_missing_returns_none() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = KeyStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let store_id = store_id_from_salt(&generate_store_salt());
        let loaded = store.load_key(&store_id).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_key_store_delete() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = KeyStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let store_id = store_id_from_salt(&generate_store_salt());
        let key = vec![7u8; KEY_SIZE];
        store.store_key(&store_id, &key).unwrap();
        assert!(store.load_key(&store_id).unwrap().is_some());

        store.delete_key(&store_id).unwrap();
        assert!(store.load_key(&store_id).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_key_store_file_permissions_locked_down() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = KeyStore::with_base_dir(temp_dir.path().to_path_buf(), false);

        let salt = generate_store_salt();
        let store_id = store_id_from_salt(&salt);
        let key = derive_store_key("pp", &salt).unwrap();
        store.store_key(&store_id, &key).unwrap();

        let dir = temp_dir.path().join("sync-keys");
        let file = dir.join(format!("{store_id}.key"));
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn test_key_store_default_constructs() {
        // Smoke test: the Default impl constructs a file-backed store.
        let _store = KeyStore::default();
    }
}

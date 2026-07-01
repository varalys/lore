//! Serverless git-ref sync for Lore.
//!
//! This module stores AI reasoning history in the user's own git repository
//! under `refs/lore/*`, with no hosted service. A lore store ref points at a
//! commit whose tree holds one encrypted blob per session plus a plaintext
//! salt, so the reasoning rides along with the code over plain git.
//!
//! The module is intentionally split into focused submodules:
//!
//! - [`encryption`] - Argon2id key derivation and AES-256-GCM encryption on
//!   raw bytes.
//! - [`keystore`] - passphrase-to-key derivation, salt generation, and
//!   persistence of the derived key (file or OS keychain).
//! - [`store`] - the consolidated session-blob pipeline: serialize a full
//!   reasoning record, gzip it, encrypt it, and the inverse.
//! - [`gitref`] - git plumbing (shelling out to the user's `git` binary) for
//!   reading and writing `refs/lore/*`.
//!
//! Only the foundational layers are implemented here. The CLI command, the
//! global personal store, and daemon wiring are built in later phases on top of
//! these primitives.

pub mod encryption;
pub mod gitref;
pub mod keystore;
pub mod store;

/// Errors produced by the git-ref sync subsystem.
///
/// A single error type spans the encryption, key storage, blob pipeline, and
/// git plumbing layers so callers can propagate failures with a single `?`.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Encryption or decryption failed.
    #[error("Encryption error: {0}")]
    Encryption(String),

    /// Storing, loading, or deleting the encryption key failed.
    #[error("Key storage error: {0}")]
    KeyStorage(String),

    /// Serializing or deserializing a session record failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Gzip compression or decompression failed.
    #[error("Compression error: {0}")]
    Compression(String),

    /// A shelled-out `git` command failed.
    #[error("Git command failed: {0}")]
    Git(String),

    /// A checked (compare-and-swap) ref update was rejected because the ref did
    /// not hold the expected old value. A caller may re-read and retry.
    #[error("Ref update rejected (concurrent change): {0}")]
    RefCasMismatch(String),

    /// An underlying I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_error_display_encryption() {
        let err = SyncError::Encryption("bad key".to_string());
        assert!(err.to_string().contains("bad key"));
    }

    #[test]
    fn test_sync_error_display_git() {
        let err = SyncError::Git("not a repository".to_string());
        assert!(err.to_string().contains("not a repository"));
    }

    #[test]
    fn test_sync_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: SyncError = io_err.into();
        assert!(matches!(err, SyncError::Io(_)));
    }
}

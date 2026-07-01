//! Consolidated session-blob pipeline for git-ref sync.
//!
//! A [`SessionRecord`] holds the complete reasoning record for a single
//! session: the session row itself plus its messages, commit links, tags,
//! annotations, and optional summary. Unlike the legacy cloud sync (which only
//! synced messages), encrypting the full record means a teammate who pulls the
//! repo can run `lore blame` and recover the commit-to-reasoning linkage.
//!
//! The on-disk blob pipeline is `serde_json -> gzip -> encrypt`. Compression
//! happens before encryption because ciphertext does not compress. The output
//! is raw bytes suitable for writing directly as a git blob (no base64).

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use super::encryption::{decrypt_data, encrypt_data};
use super::SyncError;
use crate::storage::models::{Annotation, Message, Session, SessionLink, Summary, Tag};

/// The complete reasoning record for a single session.
///
/// This is the unit that gets serialized, compressed, encrypted, and written as
/// a single git blob (`sessions/<uuid>.enc`). It is reconstructed verbatim on
/// the receiving machine so the full reasoning history, including commit links,
/// rides along with the code.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The session metadata row.
    pub session: Session,

    /// All messages belonging to the session, in conversation order.
    pub messages: Vec<Message>,

    /// Links from the session to git commits, branches, or pull requests.
    pub links: Vec<SessionLink>,

    /// User-applied tags categorizing the session.
    pub tags: Vec<Tag>,

    /// User-created annotations attached to the session.
    pub annotations: Vec<Annotation>,

    /// The session summary, if one has been generated.
    pub summary: Option<Summary>,
}

/// Manual `Debug` that prints only non-sensitive metadata.
///
/// The derived `Debug` would print plaintext message content, annotation text,
/// and summary text. Because a `SessionRecord` holds the full decrypted
/// reasoning history, a future failure log that formatted one could leak private
/// reasoning. This impl emits only counts and identifiers, never user content.
impl std::fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRecord")
            .field("session_id", &self.session.id)
            .field("tool", &self.session.tool)
            .field("message_count", &self.messages.len())
            .field("link_count", &self.links.len())
            .field("tag_count", &self.tags.len())
            .field("annotation_count", &self.annotations.len())
            .field("has_summary", &self.summary.is_some())
            .finish()
    }
}

/// Serializes and encrypts a session record into a git-blob-ready byte buffer.
///
/// The pipeline is `serde_json -> gzip -> encrypt_data`. The returned bytes are
/// the raw blob contents to hand to `git hash-object`.
///
/// # Arguments
///
/// * `record` - The full reasoning record to encode
/// * `key` - The 32-byte encryption key derived from the store passphrase
pub fn encrypt_session_record(record: &SessionRecord, key: &[u8]) -> Result<Vec<u8>, SyncError> {
    let json = serde_json::to_vec(record)
        .map_err(|e| SyncError::Serialization(format!("Failed to serialize record: {e}")))?;

    let compressed = gzip_compress(&json)?;

    encrypt_data(&compressed, key)
}

/// Decrypts and deserializes a session record from git-blob bytes.
///
/// Inverse of [`encrypt_session_record`]: `decrypt_data -> gunzip ->
/// serde_json`.
///
/// # Arguments
///
/// * `blob` - The raw blob bytes as read from `git cat-file blob`
/// * `key` - The 32-byte encryption key derived from the store passphrase
pub fn decrypt_session_record(blob: &[u8], key: &[u8]) -> Result<SessionRecord, SyncError> {
    let compressed = decrypt_data(blob, key)?;

    let json = gzip_decompress(&compressed)?;

    serde_json::from_slice(&json)
        .map_err(|e| SyncError::Serialization(format!("Failed to deserialize record: {e}")))
}

/// Compresses bytes with gzip at the default compression level.
fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, SyncError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| SyncError::Compression(format!("Gzip write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| SyncError::Compression(format!("Gzip finish failed: {e}")))
}

/// Decompresses gzip bytes produced by [`gzip_compress`].
fn gzip_decompress(data: &[u8]) -> Result<Vec<u8>, SyncError> {
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| SyncError::Compression(format!("Gzip read failed: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{LinkCreator, LinkType, MessageContent, MessageRole};
    use crate::sync::encryption::{derive_key, generate_salt};
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_record() -> SessionRecord {
        let session_id = Uuid::new_v4();
        let session = Session {
            id: session_id,
            tool: "claude-code".to_string(),
            tool_version: Some("2.0.0".to_string()),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            model: Some("claude-opus".to_string()),
            working_directory: "/home/user/project".to_string(),
            git_branch: Some("main".to_string()),
            source_path: Some("/sessions/a.jsonl".to_string()),
            message_count: 2,
            machine_id: Some("machine-1".to_string()),
        };

        let messages = vec![
            Message {
                id: Uuid::new_v4(),
                session_id,
                parent_id: None,
                index: 0,
                timestamp: Utc::now(),
                role: MessageRole::User,
                content: MessageContent::Text("Fix the bug".to_string()),
                model: None,
                git_branch: Some("main".to_string()),
                cwd: Some("/home/user/project".to_string()),
            },
            Message {
                id: Uuid::new_v4(),
                session_id,
                parent_id: None,
                index: 1,
                timestamp: Utc::now(),
                role: MessageRole::Assistant,
                content: MessageContent::Text("Done.".to_string()),
                model: Some("claude-opus".to_string()),
                git_branch: Some("main".to_string()),
                cwd: Some("/home/user/project".to_string()),
            },
        ];

        let links = vec![SessionLink {
            id: Uuid::new_v4(),
            session_id,
            link_type: LinkType::Commit,
            commit_sha: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            remote: Some("origin".to_string()),
            created_at: Utc::now(),
            created_by: LinkCreator::User,
            confidence: Some(0.95),
        }];

        let tags = vec![Tag {
            id: Uuid::new_v4(),
            session_id,
            label: "bug-fix".to_string(),
            created_at: Utc::now(),
        }];

        let annotations = vec![Annotation {
            id: Uuid::new_v4(),
            session_id,
            content: "Important fix".to_string(),
            created_at: Utc::now(),
        }];

        let summary = Some(Summary {
            id: Uuid::new_v4(),
            session_id,
            content: "Fixed a bug in the parser".to_string(),
            generated_at: Utc::now(),
        });

        SessionRecord {
            session,
            messages,
            links,
            tags,
            annotations,
            summary,
        }
    }

    #[test]
    fn test_gzip_roundtrip() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(100);
        let compressed = gzip_compress(&data).unwrap();
        let decompressed = gzip_decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_gzip_compresses_repetitive_data() {
        // Highly repetitive data should shrink under gzip.
        let data = vec![b'a'; 10_000];
        let compressed = gzip_compress(&data).unwrap();
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_encrypt_decrypt_record_roundtrip() {
        let salt = generate_salt();
        let key = derive_key("test passphrase", &salt).unwrap();

        let record = sample_record();
        let blob = encrypt_session_record(&record, &key).unwrap();
        let restored = decrypt_session_record(&blob, &key).unwrap();

        assert_eq!(restored.session.id, record.session.id);
        assert_eq!(restored.messages.len(), record.messages.len());
        assert_eq!(restored.messages[0].content.text(), "Fix the bug");
        assert_eq!(restored.links.len(), 1);
        assert_eq!(restored.links[0].commit_sha, Some("abc123".to_string()));
        assert_eq!(restored.tags[0].label, "bug-fix");
        assert_eq!(restored.annotations[0].content, "Important fix");
        assert_eq!(
            restored.summary.unwrap().content,
            "Fixed a bug in the parser"
        );
    }

    #[test]
    fn test_full_record_serialization_preserves_all_fields() {
        // Serialize to JSON and back without the crypto layer to verify the
        // record type captures every part of the reasoning record.
        let record = sample_record();
        let json = serde_json::to_vec(&record).unwrap();
        let restored: SessionRecord = serde_json::from_slice(&json).unwrap();

        assert_eq!(restored.session.tool, "claude-code");
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.links.len(), 1);
        assert_eq!(restored.tags.len(), 1);
        assert_eq!(restored.annotations.len(), 1);
        assert!(restored.summary.is_some());
    }

    #[test]
    fn test_decrypt_record_wrong_key_fails() {
        let salt = generate_salt();
        let key = derive_key("passphrase1", &salt).unwrap();
        let wrong_key = derive_key("passphrase2", &salt).unwrap();

        let record = sample_record();
        let blob = encrypt_session_record(&record, &key).unwrap();

        let result = decrypt_session_record(&blob, &wrong_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_debug_does_not_leak_plaintext() {
        // The manual Debug impl must not expose message content, annotation
        // text, or summary text, which would leak private reasoning into logs.
        let record = sample_record();
        let debug = format!("{record:?}");

        assert!(!debug.contains("Fix the bug"));
        assert!(!debug.contains("Important fix"));
        assert!(!debug.contains("Fixed a bug in the parser"));

        // It should still surface harmless metadata for diagnostics.
        assert!(debug.contains("SessionRecord"));
        assert!(debug.contains("claude-code"));
        assert!(debug.contains("message_count"));
        assert!(debug.contains("has_summary"));
    }

    /// Initializes a temp git repo for tests that cross the git blob boundary.
    ///
    /// Signing is irrelevant here (no commits are made), but identity is set for
    /// consistency with the gitref test helpers.
    fn init_test_repo(repo: &std::path::Path) {
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Lore Test"],
            vec!["config", "user.email", "test@example.com"],
        ] {
            let status = std::process::Command::new("git")
                .current_dir(repo)
                .args(&args)
                .status()
                .expect("failed to spawn git");
            assert!(status.success(), "git {args:?} failed");
        }
    }

    #[test]
    fn test_cross_module_round_trip_through_git_blob() {
        use crate::sync::gitref;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_test_repo(repo);

        let salt = generate_salt();
        let key = derive_key("cross module passphrase", &salt).unwrap();

        let record = sample_record();
        let blob = encrypt_session_record(&record, &key).unwrap();

        // Cross the real git object boundary: hash-object then cat-file.
        let sha = gitref::write_blob(repo, &blob).unwrap();
        let read_back = gitref::read_blob(repo, &sha).unwrap();
        assert_eq!(read_back, blob, "git blob round-trip must be byte-exact");

        let restored = decrypt_session_record(&read_back, &key).unwrap();
        assert_eq!(restored.session.id, record.session.id);
        assert_eq!(restored.messages.len(), record.messages.len());
        assert_eq!(restored.messages[0].content.text(), "Fix the bug");
        assert_eq!(restored.links[0].commit_sha, Some("abc123".to_string()));
        assert_eq!(
            restored.summary.unwrap().content,
            "Fixed a bug in the parser"
        );
    }

    #[test]
    fn test_binary_blob_round_trip_through_git() {
        use crate::sync::gitref;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_test_repo(repo);

        // Bytes that would be mangled by trimming or a UTF-8 decode of cat-file
        // output: leading/trailing NUL, embedded newlines, and high-bit bytes.
        let fixture: Vec<u8> = vec![
            0x00, 0x0a, 0x0d, 0xff, 0x80, b'a', 0x00, 0x81, 0xfe, 0x0a, 0x20, 0x00,
        ];

        let sha = gitref::write_blob(repo, &fixture).unwrap();
        let read_back = gitref::read_blob(repo, &sha).unwrap();
        assert_eq!(read_back, fixture, "binary blob must survive byte-for-byte");
    }

    #[test]
    fn test_record_with_no_summary() {
        let salt = generate_salt();
        let key = derive_key("passphrase", &salt).unwrap();

        let mut record = sample_record();
        record.summary = None;
        record.links.clear();
        record.tags.clear();
        record.annotations.clear();

        let blob = encrypt_session_record(&record, &key).unwrap();
        let restored = decrypt_session_record(&blob, &key).unwrap();

        assert!(restored.summary.is_none());
        assert!(restored.links.is_empty());
        assert!(restored.tags.is_empty());
        assert!(restored.annotations.is_empty());
    }
}

//! Cloud command - sync sessions with Lore cloud service.
//!
//! Provides subcommands for checking sync status, pushing sessions to the
//! cloud, and pulling sessions from other machines.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use colored::Colorize;
use serde::Serialize;
use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;

use crate::cli::OutputFormat;
use crate::cloud::client::{CloudClient, PushSession, SessionMetadata};
use crate::cloud::credentials::{require_login, CredentialsStore};
use crate::cloud::encryption::{
    decode_base64, decode_key_hex, decrypt_data, derive_key, encode_base64, encode_key_hex,
    encrypt_data,
};
use crate::config::Config;
use crate::storage::models::{Message, Session};
use crate::storage::Database;

/// Arguments for the cloud command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore cloud status          Show cloud sync status\n    \
    lore cloud push            Push local sessions to cloud\n    \
    lore cloud pull            Pull sessions from cloud\n    \
    lore cloud sync            Pull then push (bidirectional sync)")]
pub struct Args {
    #[command(subcommand)]
    pub command: CloudSubcommand,
}

/// Cloud subcommands.
#[derive(clap::Subcommand)]
pub enum CloudSubcommand {
    /// Show cloud sync status
    #[command(
        long_about = "Shows the current cloud sync status including session count,\n\
        storage used, and last sync time. Also shows how many local\n\
        sessions are pending sync."
    )]
    Status {
        /// Output format: text (default), json
        #[arg(short, long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Push local sessions to the cloud
    #[command(
        long_about = "Uploads sessions that have not been synced to the cloud.\n\
        Session messages are encrypted locally before upload using your\n\
        encryption passphrase. On first push, you will be prompted to\n\
        create a passphrase."
    )]
    Push {
        /// Show what would be pushed without actually pushing
        #[arg(long)]
        dry_run: bool,
    },

    /// Pull sessions from the cloud
    #[command(
        long_about = "Downloads sessions from the cloud that were created on other\n\
        machines. Requires your encryption passphrase to decrypt the\n\
        session content."
    )]
    Pull {
        /// Pull all sessions, not just since last sync
        #[arg(long)]
        all: bool,
    },

    /// Sync sessions with the cloud (pull then push)
    #[command(long_about = "Performs a full bidirectional sync with the cloud.\n\
        First pulls any new sessions from other machines, then pushes\n\
        local sessions that haven't been synced yet.")]
    Sync,
}

/// JSON output for cloud status.
#[derive(Serialize)]
struct StatusOutput {
    logged_in: bool,
    email: Option<String>,
    plan: Option<String>,
    cloud: Option<CloudStatus>,
    local: LocalStatus,
}

#[derive(Serialize)]
struct CloudStatus {
    session_count: i64,
    storage_used_bytes: i64,
    last_sync_at: Option<String>,
}

#[derive(Serialize)]
struct LocalStatus {
    total_sessions: i32,
    unsynced_sessions: i32,
    last_sync_at: Option<String>,
}

/// Executes the cloud command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        CloudSubcommand::Status { format } => run_status(format),
        CloudSubcommand::Push { dry_run } => run_push(dry_run),
        CloudSubcommand::Pull { all } => run_pull(all),
        CloudSubcommand::Sync => run_sync(),
    }
}

/// Shows cloud sync status.
fn run_status(format: OutputFormat) -> Result<()> {
    let db = Database::open_default()?;
    let config = Config::load()?;
    let store = CredentialsStore::with_keychain(config.use_keychain);
    let creds = store.load().context("Failed to check login status")?;

    let total_sessions = db.session_count()?;
    let unsynced_sessions = db.unsynced_session_count()?;
    let last_local_sync = db.last_sync_time()?;

    match format {
        OutputFormat::Json => {
            let cloud_status = if let Some(ref creds) = creds {
                let client = CloudClient::with_url(&creds.cloud_url).with_api_key(&creds.api_key);
                match client.status() {
                    Ok(status) => Some(CloudStatus {
                        session_count: status.session_count,
                        storage_used_bytes: status.storage_used_bytes,
                        last_sync_at: status.last_sync_at.map(|t| t.to_rfc3339()),
                    }),
                    Err(e) => {
                        tracing::debug!("Failed to get cloud status: {e}");
                        None
                    }
                }
            } else {
                None
            };

            let output = StatusOutput {
                logged_in: creds.is_some(),
                email: creds.as_ref().map(|c| c.email.clone()),
                plan: creds.as_ref().map(|c| c.plan.clone()),
                cloud: cloud_status,
                local: LocalStatus {
                    total_sessions,
                    unsynced_sessions,
                    last_sync_at: last_local_sync.map(|t| t.to_rfc3339()),
                },
            };

            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!("{}", "Cloud Sync".bold());
            println!();

            match creds {
                Some(creds) => {
                    println!("{}", "Account:".bold());
                    println!("  Email: {}", creds.email.cyan());
                    println!("  Plan:  {}", creds.plan);
                    println!();

                    // Get cloud status
                    let client =
                        CloudClient::with_url(&creds.cloud_url).with_api_key(&creds.api_key);
                    match client.status() {
                        Ok(status) => {
                            println!("{}", "Cloud:".bold());
                            println!("  Sessions: {}", status.session_count);
                            println!("  Storage:  {}", format_bytes(status.storage_used_bytes));
                            if let Some(last_sync) = status.last_sync_at {
                                println!("  Last sync: {}", format_relative_time(last_sync));
                            }
                            println!();
                        }
                        Err(e) => {
                            println!("{}: {}", "Cloud status unavailable".yellow(), e);
                            println!();
                        }
                    }
                }
                None => {
                    println!(
                        "{} Run 'lore login' to authenticate.",
                        "Not logged in.".yellow()
                    );
                    println!();
                }
            }

            println!("{}", "Local:".bold());
            println!("  Total sessions:    {}", total_sessions);
            println!("  Pending sync:      {}", unsynced_sessions);
            if let Some(last_sync) = last_local_sync {
                println!("  Last sync:         {}", format_relative_time(last_sync));
            }
        }
    }

    Ok(())
}

/// Number of sessions to include in each batch when pushing to the cloud.
/// Kept small to avoid 413 errors from large sessions with many messages.
const PUSH_BATCH_SIZE: usize = 3;

/// Pushes local sessions to the cloud.
fn run_push(dry_run: bool) -> Result<()> {
    let creds = require_login()?;
    let db = Database::open_default()?;
    let config = Config::load()?;
    let client = CloudClient::with_url(&creds.cloud_url).with_api_key(&creds.api_key);

    // Ensure salt is synced to cloud (migration for existing users)
    if let Some(ref local_salt) = config.encryption_salt {
        match client.get_salt() {
            Ok(None) => {
                // Cloud doesn't have salt, upload it
                if let Err(e) = client.set_salt(local_salt) {
                    tracing::debug!("Could not sync salt to cloud: {e}");
                } else {
                    tracing::debug!("Synced encryption salt to cloud");
                }
            }
            Ok(Some(_)) => {
                // Cloud already has salt, nothing to do
            }
            Err(e) => {
                tracing::debug!("Could not check cloud salt: {e}");
            }
        }
    }

    // Get unsynced sessions
    let sessions = db.get_unsynced_sessions()?;
    if sessions.is_empty() {
        println!("{}", "All sessions are already synced.".green());
        return Ok(());
    }

    println!("Found {} sessions to sync.", sessions.len());

    if dry_run {
        println!();
        println!("{}", "Dry run - would push:".yellow());
        for session in &sessions {
            println!(
                "  {} ({}, {} messages)",
                &session.id.to_string()[..8],
                session.tool,
                session.message_count
            );
        }
        return Ok(());
    }

    // Get or create encryption key
    let store = CredentialsStore::with_keychain(config.use_keychain);
    let mut config = config; // Make mutable for potential salt creation
    let encryption_key = match store.load_encryption_key()? {
        Some(key_hex) => decode_key_hex(&key_hex)?,
        None => {
            // First push - prompt for passphrase
            println!();
            println!("{}", "First sync - set up encryption".bold());
            println!(
                "Your session content will be encrypted with a passphrase that only you know."
            );
            println!("The cloud service cannot read your session content.");
            println!();

            let passphrase = prompt_new_passphrase()?;
            let salt_b64 = config.get_or_create_encryption_salt()?;
            let salt = BASE64.decode(&salt_b64)?;
            let key = derive_key(&passphrase, &salt)?;

            // Store the derived key (not the passphrase)
            store.store_encryption_key(&encode_key_hex(&key))?;

            // Sync salt to cloud for other machines
            if let Err(e) = client.set_salt(&salt_b64) {
                tracing::debug!("Could not sync salt to cloud (may already exist): {e}");
            }

            key
        }
    };

    // Get machine ID
    let machine_id = config.get_or_create_machine_id()?;

    // Pre-read all messages (fast - just SELECT queries)
    println!();
    print!("  Reading sessions...");
    io::stdout().flush()?;
    let session_data: Vec<_> = sessions
        .iter()
        .map(|session| {
            let messages = db.get_messages(&session.id)?;
            Ok((session.clone(), messages))
        })
        .collect::<Result<Vec<_>>>()?;
    println!(" done");

    // Split into batches for processing
    let batches: Vec<Vec<_>> = session_data
        .chunks(PUSH_BATCH_SIZE)
        .map(|chunk| chunk.to_vec())
        .collect();
    let total_batches = batches.len();

    // Channel for encrypted batches (bounded to 2 for backpressure)
    let (tx, rx) = mpsc::sync_channel::<Result<(usize, Vec<PushSession>)>>(2);

    // Spawn encryption thread
    let encrypt_handle = thread::spawn(move || {
        for (batch_idx, batch) in batches.into_iter().enumerate() {
            let mut push_sessions = Vec::new();
            for (session, messages) in batch {
                let encrypted = match encrypt_session_messages(&messages, &encryption_key) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                push_sessions.push(PushSession {
                    id: session.id.to_string(),
                    machine_id: machine_id.clone(),
                    encrypted_data: encrypted,
                    metadata: SessionMetadata {
                        tool_name: session.tool.clone(),
                        project_path: session.working_directory.clone(),
                        started_at: session.started_at,
                        ended_at: session.ended_at,
                        message_count: session.message_count,
                    },
                    updated_at: session.ended_at.unwrap_or_else(Utc::now),
                });
            }
            if tx.send(Ok((batch_idx, push_sessions))).is_err() {
                return; // Receiver dropped, stop processing
            }
        }
    });

    // Main thread: receive encrypted batches and upload (pipelined)
    println!("  Encrypting and uploading ({} batches)...", total_batches);
    let mut total_synced: i64 = 0;
    let mut batch_errors: Vec<(usize, String)> = Vec::new();
    let mut too_large_sessions: Vec<String> = Vec::new();

    for received in rx {
        let (batch_idx, batch) = received?;
        let batch_num = batch_idx + 1;
        print!("    Batch {}/{}... ", batch_num, total_batches);
        io::stdout().flush()?;

        match client.push(batch.to_vec()) {
            Ok(response) => {
                println!("done");

                // Mark sessions in this batch as synced immediately
                let batch_session_ids: Vec<_> = batch
                    .iter()
                    .filter_map(|ps| uuid::Uuid::parse_str(&ps.id).ok())
                    .collect();
                db.mark_sessions_synced(&batch_session_ids, response.server_time)?;

                total_synced += response.synced_count;
            }
            Err(e) => {
                let error_str = e.to_string();

                // Check if this is a size-related error (413 or "Too Large")
                if is_size_error(&error_str) {
                    println!("{}", "failed (retrying individually)".yellow());

                    // Retry each session in the batch individually
                    for session in &batch {
                        let session_short_id = &session.id[..8];
                        print!("      Session {}... ", session_short_id);
                        io::stdout().flush()?;

                        match client.push(vec![session.clone()]) {
                            Ok(response) => {
                                println!("done");
                                if let Ok(session_id) = uuid::Uuid::parse_str(&session.id) {
                                    db.mark_sessions_synced(&[session_id], response.server_time)?;
                                }
                                total_synced += response.synced_count;
                            }
                            Err(individual_err) => {
                                let individual_error_str = individual_err.to_string();
                                if is_size_error(&individual_error_str) {
                                    println!("{}", "too large, skipping".yellow());
                                    too_large_sessions.push(session.id.clone());
                                } else {
                                    println!("{}", "failed".red());
                                    batch_errors.push((
                                        batch_num,
                                        format!(
                                            "Session {}: {}",
                                            session_short_id, individual_error_str
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                } else {
                    println!("{}", "failed".red());
                    batch_errors.push((batch_num, error_str));
                }
            }
        }
    }

    // Wait for encryption thread to finish
    encrypt_handle.join().expect("Encryption thread panicked");

    println!();

    // Report results
    if batch_errors.is_empty() && too_large_sessions.is_empty() {
        println!(
            "{} Synced {} sessions to the cloud.",
            "Success!".green().bold(),
            total_synced
        );
    } else if batch_errors.is_empty() {
        // Only size issues, no other errors
        println!(
            "{} Synced {} sessions to the cloud.",
            "Success!".green().bold(),
            total_synced
        );
        println!(
            "{} {} session(s) were too large to sync:",
            "Note:".yellow(),
            too_large_sessions.len()
        );
        for session_id in &too_large_sessions {
            println!("  {}", &session_id[..8]);
        }
    } else {
        // Some batches failed with non-size errors
        if total_synced > 0 {
            println!(
                "{} Synced {} sessions, but {} error(s) occurred:",
                "Partial success.".yellow().bold(),
                total_synced,
                batch_errors.len()
            );
        } else {
            println!("{} All batches failed:", "Error!".red().bold());
        }
        for (batch_num, error) in &batch_errors {
            println!("  Batch {}: {}", batch_num, error);
        }
        if !too_large_sessions.is_empty() {
            println!(
                "{} {} session(s) were too large to sync:",
                "Note:".yellow(),
                too_large_sessions.len()
            );
            for session_id in &too_large_sessions {
                println!("  {}", &session_id[..8]);
            }
        }
    }

    Ok(())
}

/// Pulls sessions from the cloud.
fn run_pull(all: bool) -> Result<()> {
    let creds = require_login()?;
    let mut db = Database::open_default()?;

    // Determine since time
    let since = if all { None } else { db.last_sync_time()? };

    // Create client early so we can fetch salt if needed
    let client = CloudClient::with_url(&creds.cloud_url).with_api_key(&creds.api_key);

    // Get encryption key
    let mut config = Config::load()?;
    let store = CredentialsStore::with_keychain(config.use_keychain);
    let encryption_key = match store.load_encryption_key()? {
        Some(key_hex) => decode_key_hex(&key_hex)?,
        None => {
            // Need to prompt for passphrase
            println!("Enter your encryption passphrase to decrypt sessions:");
            let passphrase = prompt_passphrase()?;

            // Try to get salt from local config first, then from cloud
            let salt_b64 = match &config.encryption_salt {
                Some(salt) => salt.clone(),
                None => {
                    // Fetch salt from cloud
                    let cloud_salt = client.get_salt()?.ok_or_else(|| {
                        anyhow::anyhow!(
                            "No encryption salt found locally or on cloud. Run 'lore cloud push' on a machine with existing sessions first."
                        )
                    })?;
                    // Save salt locally for future use
                    config.encryption_salt = Some(cloud_salt.clone());
                    config.save()?;
                    cloud_salt
                }
            };
            let salt = BASE64.decode(&salt_b64)?;
            let key = derive_key(&passphrase, &salt)?;

            // Store for future use
            store.store_encryption_key(&encode_key_hex(&key))?;
            key
        }
    };

    println!("Downloading sessions from cloud...");
    let response = client.pull(since)?;

    if response.sessions.is_empty() {
        println!("{}", "No new sessions to pull.".green());
        return Ok(());
    }

    println!("Found {} sessions to process.", response.sessions.len());

    let mut imported = 0;
    let mut updated = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let total = response.sessions.len();
    let config = Config::load()?;
    let local_machine_id = config.machine_id.clone();

    for (idx, pull_session) in response.sessions.into_iter().enumerate() {
        // Progress indicator - use eprint to ensure immediate output
        eprint!("\r  Processing sessions... {}/{}", idx + 1, total);

        // Skip sessions from this machine (we already have them)
        if Some(&pull_session.machine_id) == local_machine_id.as_ref() {
            skipped += 1;
            continue;
        }

        // Check if session already exists and whether cloud version is newer
        let existing_session = db
            .find_session_by_id_prefix(&pull_session.id)
            .ok()
            .flatten();
        let is_update = if let Some(ref existing) = existing_session {
            // Cloud version is newer if it has more messages or a later ended_at
            let cloud_has_more_messages =
                pull_session.metadata.message_count > existing.message_count;
            let cloud_has_later_ended_at = match (pull_session.metadata.ended_at, existing.ended_at)
            {
                (Some(cloud_end), Some(local_end)) => cloud_end > local_end,
                (Some(_), None) => true, // Cloud has ended_at, local does not
                _ => false,
            };
            cloud_has_more_messages || cloud_has_later_ended_at
        } else {
            false
        };

        // Skip if session exists and cloud version is not newer
        if existing_session.is_some() && !is_update {
            skipped += 1;
            continue;
        }

        // Decrypt messages
        let messages = match decrypt_session_messages(&pull_session.encrypted_data, &encryption_key)
        {
            Ok(msgs) => msgs,
            Err(e) => {
                failed += 1;
                tracing::debug!("Failed to decrypt session {}: {}", &pull_session.id[..8], e);
                continue;
            }
        };

        let session_id = uuid::Uuid::parse_str(&pull_session.id).context("Invalid session ID")?;

        let session = Session {
            id: session_id,
            tool: pull_session.metadata.tool_name,
            tool_version: None,
            started_at: pull_session.metadata.started_at,
            ended_at: pull_session.metadata.ended_at,
            model: None,
            working_directory: pull_session.metadata.project_path,
            git_branch: None,
            source_path: None,
            message_count: pull_session.metadata.message_count,
            machine_id: Some(pull_session.machine_id),
        };

        // Import session and all messages in a single transaction
        // This handles both new inserts and updates via ON CONFLICT
        db.import_session_with_messages(&session, &messages, Some(response.server_time))?;

        if is_update {
            updated += 1;
        } else {
            imported += 1;
        }
    }

    // Clear the progress line and print summary
    eprintln!();
    if failed > 0 {
        println!(
            "{} Imported {} sessions, updated {} ({} skipped, {} failed to decrypt).",
            "Done.".yellow().bold(),
            imported,
            updated,
            skipped,
            failed
        );
    } else if updated > 0 {
        println!(
            "{} Imported {} sessions, updated {} ({} skipped).",
            "Success!".green().bold(),
            imported,
            updated,
            skipped
        );
    } else {
        println!(
            "{} Imported {} sessions ({} skipped).",
            "Success!".green().bold(),
            imported,
            skipped
        );
    }

    Ok(())
}

/// Syncs sessions with the cloud (pull then push).
fn run_sync() -> Result<()> {
    println!("{}", "Cloud Sync".bold());
    println!();

    // Pull first to get any remote changes
    println!("{}", "Step 1: Pull".bold());
    if let Err(e) = run_pull(false) {
        // Don't fail the whole sync if pull fails, but warn
        println!("{} Pull failed: {}", "Warning:".yellow(), e);
        println!("Continuing with push...");
        println!();
    }

    println!();

    // Then push local changes
    println!("{}", "Step 2: Push".bold());
    run_push(false)?;

    Ok(())
}

/// Encrypts session messages for cloud storage.
fn encrypt_session_messages(messages: &[Message], key: &[u8]) -> Result<String> {
    let json = serde_json::to_vec(messages)?;
    let encrypted = encrypt_data(&json, key)?;
    Ok(encode_base64(&encrypted))
}

/// Decrypts session messages from cloud storage.
fn decrypt_session_messages(encrypted_b64: &str, key: &[u8]) -> Result<Vec<Message>> {
    let encrypted = decode_base64(encrypted_b64)?;
    let decrypted = decrypt_data(&encrypted, key)?;
    let messages: Vec<Message> = serde_json::from_slice(&decrypted)?;
    Ok(messages)
}

/// Prompts for a new passphrase (with confirmation).
fn prompt_new_passphrase() -> Result<String> {
    loop {
        print!("Enter passphrase: ");
        io::stdout().flush()?;
        let passphrase = rpassword::read_password()?;

        if passphrase.len() < 8 {
            println!("{}", "Passphrase must be at least 8 characters.".red());
            continue;
        }

        print!("Confirm passphrase: ");
        io::stdout().flush()?;
        let confirm = rpassword::read_password()?;

        if passphrase != confirm {
            println!("{}", "Passphrases do not match.".red());
            continue;
        }

        return Ok(passphrase);
    }
}

/// Prompts for an existing passphrase.
fn prompt_passphrase() -> Result<String> {
    print!("Passphrase: ");
    io::stdout().flush()?;
    let passphrase = rpassword::read_password()?;
    Ok(passphrase)
}

/// Formats bytes as human-readable size.
fn format_bytes(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Checks if an error message indicates a payload size issue (413 or "Too Large").
fn is_size_error(error_msg: &str) -> bool {
    error_msg.contains("413")
        || error_msg.to_lowercase().contains("too large")
        || error_msg.to_lowercase().contains("payload")
}

/// Formats a timestamp as relative time.
fn format_relative_time(time: chrono::DateTime<chrono::Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(time);

    let hours = duration.num_hours();
    if hours < 1 {
        let minutes = duration.num_minutes();
        if minutes < 1 {
            "just now".to_string()
        } else {
            format!("{} minutes ago", minutes)
        }
    } else if hours < 24 {
        format!("{} hours ago", hours)
    } else {
        let days = duration.num_days();
        format!("{} days ago", days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn test_is_size_error() {
        // Should detect 413 status code
        assert!(is_size_error("HTTP error: 413 Payload Too Large"));
        assert!(is_size_error("Server returned 413"));

        // Should detect "too large" text (case insensitive)
        assert!(is_size_error("Request body too large"));
        assert!(is_size_error("Session Too Large to upload"));

        // Should detect payload-related errors
        assert!(is_size_error("Payload size exceeded"));
        assert!(is_size_error("Request payload too big"));

        // Should not match unrelated errors
        assert!(!is_size_error("Connection refused"));
        assert!(!is_size_error("HTTP error: 500 Internal Server Error"));
        assert!(!is_size_error("Authentication failed"));
        assert!(!is_size_error("Network timeout"));
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        use crate::cloud::encryption::generate_salt;
        use crate::storage::models::{MessageContent, MessageRole};
        use chrono::Utc;
        use uuid::Uuid;

        let salt = generate_salt();
        let key = derive_key("test passphrase", &salt).unwrap();

        let messages = vec![Message {
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
        }];

        let encrypted = encrypt_session_messages(&messages, &key).unwrap();
        let decrypted = decrypt_session_messages(&encrypted, &key).unwrap();

        assert_eq!(decrypted.len(), 1);
        assert_eq!(decrypted[0].content.text(), "Hello, world!");
    }
}

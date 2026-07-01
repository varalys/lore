//! Sync command - serverless git-ref sync for a repository's lore store.
//!
//! Stores AI reasoning history in the repository's own git repo under
//! `refs/lore/sessions` instead of a hosted service. A store ref points at a
//! commit whose tree holds one encrypted blob per session (the full reasoning
//! record: messages, links, tags, annotations, and summary) plus a plaintext
//! salt and a machine registry. Sharing the repo (plain git) and a passphrase
//! (out of band) is all a teammate needs to read the reasoning.
//!
//! This module implements the per-repo store only:
//!
//! - `lore sync setup` - create or join the store for this repo, set the
//!   passphrase, and write the salt to the ref.
//! - `lore sync` - fetch, merge remote reasoning into the local database, then
//!   build, commit, and push the updated store.
//! - `lore sync status` - report whether the store is set up, the unsynced
//!   count, the last sync time, and local and remote ref state.
//!
//! All git access shells out through [`crate::sync::gitref`], inheriting the
//! user's authentication and remotes.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::config::Config;
use crate::storage::models::{Machine, Session};
use crate::storage::Database;
use crate::sync::gitref::{self, TreeEntry};
use crate::sync::keystore::{derive_store_key, generate_store_salt, store_id_from_salt, KeyStore};
use crate::sync::store::{decrypt_session_record, encrypt_session_record, SessionRecord};
use crate::sync::SyncError;

/// Full ref name for a repository's per-repo lore store.
///
/// Lives outside `refs/heads/*` so it never checks out into the working tree.
const SESSIONS_REF: &str = "refs/lore/sessions";

/// Minimum passphrase length for a newly created store.
const MIN_PASSPHRASE_LEN: usize = 8;

/// Maximum number of fetch/merge/build/push attempts before giving up.
///
/// A concurrent local sync (compare-and-swap mismatch) or a remote that moved
/// between our fetch and push (non-fast-forward) triggers a re-fetch and retry.
const MAX_SYNC_ATTEMPTS: usize = 5;

/// Arguments for the sync command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore sync setup            Create or join this repo's encrypted lore store\n    \
    lore sync                  Fetch, merge, and push reasoning history\n    \
    lore sync status           Show sync state for this repo\n    \
    lore sync --remote upstream  Sync against a non-default remote")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<SyncSubcommand>,

    /// Remote to sync the lore store with (default: origin).
    #[arg(long, global = true, default_value = "origin")]
    pub remote: String,

    /// Hook-friendly mode used by the pre-push hook.
    ///
    /// No-ops and exits 0 when this repo's store is not set up or no key is
    /// stored on this machine, never prompts for a passphrase, and keeps output
    /// minimal. Only affects a full sync (no subcommand).
    #[arg(long)]
    pub quiet: bool,
}

/// Sync subcommands. When omitted, a full sync runs.
#[derive(clap::Subcommand)]
pub enum SyncSubcommand {
    /// Set the passphrase for this repo's lore store and write its salt.
    #[command(
        long_about = "Creates a new encrypted store for this repository, or joins an\n\
        existing one already pushed to the remote. When joining, you are\n\
        prompted for the shared passphrase and it is verified against an\n\
        existing session. The derived key is stored locally so later syncs\n\
        do not prompt again."
    )]
    Setup,

    /// Show sync status for this repo's lore store.
    #[command(
        long_about = "Reports whether the store is set up, how many local sessions are\n\
        pending sync, the last sync time, and the local and remote ref state."
    )]
    Status {
        /// Output format: text (default) or json.
        #[arg(short, long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
}

/// A machine's identity for the store's machine registry.
struct MachineIdentity {
    /// Stable machine UUID.
    id: String,
    /// Human-readable machine name.
    name: String,
}

/// Plaintext per-session metadata blob (`sessions/<uuid>.meta.json`).
///
/// Written unencrypted so the store can be listed without the passphrase.
#[derive(Serialize, Deserialize)]
struct SessionMeta {
    /// Session UUID.
    id: Uuid,
    /// Tool that produced the session (for example `claude-code`).
    tool: String,
    /// When the session started.
    started_at: DateTime<Utc>,
    /// When the session ended, if it has.
    ended_at: Option<DateTime<Utc>>,
    /// Number of messages in the session.
    message_count: i32,
    /// Machine that captured the session.
    machine_id: Option<String>,
    /// Git branch active when the session was captured.
    git_branch: Option<String>,
}

/// Summary of a completed sync for reporting.
#[derive(Debug)]
struct SyncSummary {
    /// Sessions merged from the remote into the local database.
    pulled: usize,
    /// Local unsynced sessions written into the store and pushed.
    pushed: usize,
}

/// JSON output for `lore sync status`.
#[derive(Serialize)]
struct StatusOutput {
    set_up: bool,
    keyed: bool,
    unsynced_sessions: i32,
    last_sync_at: Option<String>,
    remote_store_exists: bool,
    local_ref: Option<String>,
    tracking_ref: Option<String>,
    remote: String,
}

/// Executes the sync command.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        Some(SyncSubcommand::Setup) => run_setup(&args.remote),
        Some(SyncSubcommand::Status { format }) => run_status(&args.remote, format),
        None if args.quiet => run_sync_quiet(&args.remote),
        None => run_sync(&args.remote),
    }
}

// ==================== setup ====================

/// Creates or joins the lore store for the current repository.
fn run_setup(remote: &str) -> Result<()> {
    let repo = current_repo()?;
    let mut config = Config::load()?;
    let machine = machine_identity(&mut config)?;
    let keystore = KeyStore::with_keychain(config.use_keychain);

    // Populate the remote-tracking ref so we can tell whether a store already
    // exists on the remote (and read its salt) without touching the local ref.
    gitref::fetch(&repo, remote, SESSIONS_REF)
        .with_context(|| format!("Failed to reach remote '{remote}'"))?;

    match read_store_salt(&repo, remote)? {
        Some(salt) => {
            println!("{}", "An existing lore store was found. Joining it.".bold());
            println!("Enter the shared passphrase for this repo's lore store.");
            let passphrase = prompt_passphrase()?;
            join_store(&repo, remote, &keystore, &machine, &salt, &passphrase)?;
            println!("{} Joined the lore store.", "Success!".green().bold());
        }
        None => {
            println!("{}", "Setting up a new lore store for this repo.".bold());
            println!(
                "Your reasoning history is encrypted with a passphrase only you and\n\
                 your teammates know. Share it out of band; the git host never sees it."
            );
            println!();
            let passphrase = prompt_new_passphrase()?;
            create_store(&repo, remote, &keystore, &machine, &passphrase)?;
            println!("{} Created the lore store.", "Success!".green().bold());
        }
    }

    println!("Run 'lore sync' to push your reasoning history.");
    Ok(())
}

/// Creates a brand new store: fresh salt, derived key, salt written to the ref.
fn create_store(
    repo: &Path,
    remote: &str,
    keystore: &KeyStore,
    machine: &MachineIdentity,
    passphrase: &str,
) -> Result<()> {
    let salt = generate_store_salt();
    let key = derive_store_key(passphrase, &salt)?;
    establish_store(repo, remote, keystore, machine, &salt, &key)
}

/// Joins an existing store: derive the key from the shared salt and verify it.
fn join_store(
    repo: &Path,
    remote: &str,
    keystore: &KeyStore,
    machine: &MachineIdentity,
    salt: &[u8],
    passphrase: &str,
) -> Result<()> {
    let key = derive_store_key(passphrase, salt)?;

    // Verify the passphrase by decrypting one existing session, if any exist.
    if let Some(blob) = first_session_blob(repo, remote)? {
        decrypt_session_record(&blob, &key).map_err(|_| {
            anyhow!("Wrong passphrase: could not decrypt an existing session in the store")
        })?;
    }

    establish_store(repo, remote, keystore, machine, salt, &key)
}

/// Stores the derived key and writes the store metadata to the local ref.
///
/// The new tree starts from the remote-tracking ref when present (so a join
/// adopts the remote store's sessions and salt), otherwise from the existing
/// local ref, otherwise from scratch. Only `meta/salt` and `meta/machines.json`
/// are written here; session blobs are added by [`run_sync`].
fn establish_store(
    repo: &Path,
    remote: &str,
    keystore: &KeyStore,
    machine: &MachineIdentity,
    salt: &[u8],
    key: &[u8],
) -> Result<()> {
    let store_id = store_id_from_salt(salt);
    keystore.store_key(&store_id, key)?;

    let (base, parent) = store_base(repo, remote)?;

    let mut machines = match &base {
        Some(reference) => read_machines(repo, reference)?,
        None => BTreeMap::new(),
    };
    machines.insert(machine.id.clone(), machine.name.clone());

    let mut changes = BTreeMap::new();
    changes.insert("meta/salt".to_string(), gitref::write_blob(repo, salt)?);
    changes.insert(
        "meta/machines.json".to_string(),
        gitref::write_blob(repo, &serde_json::to_vec(&machines)?)?,
    );

    let tree = gitref::build_tree(repo, base.as_deref(), &changes)?;
    let commit = gitref::commit_tree(repo, &tree, parent.as_deref(), "lore: set up store")?;
    gitref::update_ref(repo, SESSIONS_REF, &commit)?;
    gitref::push(repo, remote, SESSIONS_REF)
        .with_context(|| format!("Failed to push the lore store to '{remote}'"))?;

    // Configure a tracking-namespace fetch refspec so a plain `git pull` keeps
    // the remote-tracking lore refs fresh for this remote. The real merge into
    // the local ref still requires `lore sync`; this only pre-populates the
    // tracking refs. Best-effort: a failure here must not fail setup.
    if let Err(e) = gitref::add_lore_fetch_refspec(repo, remote) {
        tracing::debug!("Could not add lore fetch refspec for '{remote}': {e}");
    }

    Ok(())
}

// ==================== sync ====================

/// Performs a full sync of the current repository's lore store.
fn run_sync(remote: &str) -> Result<()> {
    let repo = current_repo()?;
    let mut config = Config::load()?;
    let machine = machine_identity(&mut config)?;
    let keystore = KeyStore::with_keychain(config.use_keychain);

    let (key, salt) = load_store_credentials(&repo, remote, &keystore)?;

    let mut db = Database::open_default()?;
    // Push only this repo's own sessions so cross-project history is never
    // written into (and shared through) this repo's store.
    let sessions = db.get_unsynced_sessions_for_repo(&repo)?;
    let summary = perform_sync(&mut db, &repo, remote, &key, &salt, &machine, sessions)?;

    println!(
        "{} Pulled {}, pushed {}.",
        "Sync complete.".green().bold(),
        summary.pulled,
        summary.pushed
    );
    Ok(())
}

/// Performs a hook-friendly sync used by the pre-push hook.
///
/// Skips silently (returns Ok without touching the config or database) when the
/// repo's store is not set up or no key is stored on this machine, never
/// prompts, and keeps output minimal. Sync errors are propagated so the caller
/// (the pre-push hook) can surface a brief warning; they never block the push
/// because the hook always exits 0.
fn run_sync_quiet(remote: &str) -> Result<()> {
    let repo = current_repo()?;
    sync_quiet_in_repo(&repo, remote)
}

/// Core of the quiet sync for a resolved repository path.
///
/// Split from [`run_sync_quiet`] so it can be exercised in tests with a
/// temporary repo rather than the process working directory.
fn sync_quiet_in_repo(repo: &Path, remote: &str) -> Result<()> {
    // git hands the pre-push hook the pushed remote's name, but it may be a raw
    // URL or an unknown name; resolve it to a configured remote or skip.
    let remote = match resolve_hook_remote(repo, remote)? {
        Some(remote) => remote,
        None => return Ok(()),
    };

    // Skip silently when the store is not set up. This check reads only local
    // refs, so a repo that never opted into lore sync is a no-op that never
    // touches the config or database.
    let salt = match read_store_salt(repo, &remote)? {
        Some(salt) => salt,
        None => return Ok(()),
    };

    // Resolve the key store read-only. `Config::load` never writes, so this
    // reads `use_keychain` without persisting anything; the mutating machine-id
    // step is deferred to after a key is confirmed present.
    let keystore = KeyStore::with_keychain(Config::load()?.use_keychain);

    quiet_sync_with_keystore(repo, &remote, &salt, &keystore)
}

/// Runs a quiet sync for a set-up store, given an already-resolved key store.
///
/// Returns Ok without mutating the config or opening the database when no key is
/// stored on this machine. Only after a key is confirmed present does it persist
/// this machine's id (the first mutating step), open the database, and sync.
/// Split from [`sync_quiet_in_repo`] so it can be exercised with a key store
/// isolated to a temp directory.
fn quiet_sync_with_keystore(
    repo: &Path,
    remote: &str,
    salt: &[u8],
    keystore: &KeyStore,
) -> Result<()> {
    // Skip silently when no key is stored on this machine. Quiet mode never
    // prompts for a passphrase; the user runs 'lore sync setup' interactively.
    // This precedes every mutating step, so a set-up-but-unkeyed repo never
    // writes the config or opens the database.
    let store_id = store_id_from_salt(salt);
    let key = match keystore.load_key(&store_id)? {
        Some(key) => key,
        None => return Ok(()),
    };

    // A key is present: now perform the mutating work. `machine_identity`
    // persists a generated machine id to the config, so it must not run on the
    // no-key path above.
    let mut config = Config::load()?;
    let machine = machine_identity(&mut config)?;
    let mut db = Database::open_default()?;
    let sessions = db.get_unsynced_sessions_for_repo(repo)?;
    perform_sync(&mut db, repo, remote, &key, salt, &machine, sessions)?;
    Ok(())
}

/// Resolves the remote to sync against for a hook invocation.
///
/// git passes the pushed remote's name (or a raw URL for an anonymous push) to
/// the pre-push hook. When it names a configured remote, use it. Otherwise fall
/// back to the default remote (`origin`) when configured, else return `None` so
/// the caller skips gracefully rather than erroring on an unknown remote.
fn resolve_hook_remote(repo: &Path, remote: &str) -> Result<Option<String>> {
    let remotes = configured_remotes(repo)?;
    if remotes.iter().any(|r| r == remote) {
        return Ok(Some(remote.to_string()));
    }
    if remotes.iter().any(|r| r == "origin") {
        return Ok(Some("origin".to_string()));
    }
    Ok(None)
}

/// Lists the names of remotes configured in the repository.
fn configured_remotes(repo: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["remote"])
        .output()
        .context("Failed to run git remote")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect())
}

/// Fetches, merges remote reasoning into the database, then builds and pushes.
///
/// The caller supplies `sessions`, the exact set of local sessions to push into
/// this store. For a per-repo store that is the repo-scoped unsynced set (see
/// [`Database::get_unsynced_sessions_for_repo`]) so cross-project history never
/// leaks into one repo's store; a future global store can pass all unsynced
/// sessions without any change here. Inbound merge is independent of this set:
/// every remote session is pulled regardless of its working directory.
///
/// Retries the whole cycle on a compare-and-swap mismatch (a concurrent local
/// sync moved the ref) or a non-fast-forward push (the remote moved between our
/// fetch and push), up to [`MAX_SYNC_ATTEMPTS`].
fn perform_sync(
    db: &mut Database,
    repo: &Path,
    remote: &str,
    key: &[u8],
    salt: &[u8],
    machine: &MachineIdentity,
    sessions: Vec<Session>,
) -> Result<SyncSummary> {
    let mut pulled_total = 0;

    for attempt in 0..MAX_SYNC_ATTEMPTS {
        let is_last = attempt + 1 == MAX_SYNC_ATTEMPTS;
        let old_local = gitref::resolve_ref(repo, SESSIONS_REF)?;

        // FETCH: bring the remote store into the tracking ref (None when the
        // remote store has never been initialized).
        let fetched = gitref::fetch(repo, remote, SESSIONS_REF)
            .with_context(|| format!("Failed to fetch the lore store from '{remote}'"))?;
        let tracking_entries = if fetched.is_some() {
            gitref::read_tracking_tree(repo, remote, SESSIONS_REF)?
        } else {
            Vec::new()
        };

        // MERGE remote -> local database (full records, newer-wins). A wrong key
        // surfaces here and aborts before anything is built or pushed.
        pulled_total += merge_remote(db, repo, &tracking_entries, key)?;
        merge_machines(db, repo, &tracking_entries)?;

        // BUILD the outgoing tree. Separate the TREE BASE (which entries the new
        // tree inherits) from the COMMIT PARENT (which commit it descends from):
        //
        // - Remote present: base the tree on the remote tracking commit so the
        //   push is a fast-forward and remote-only sessions are preserved. The
        //   commit descends from that same tracking commit.
        // - No remote store: base the tree on NOTHING (empty). The local ref may
        //   have been written before per-repo scoping and can hold out-of-scope
        //   session artifacts; inheriting it wholesale would re-push another
        //   repo's history. Only in-scope local-only sessions are carried
        //   forward below. The commit still descends from the local ref (when it
        //   exists) so the local ref update stays a fast-forward and history is
        //   continuous.
        let tracking_commit = match fetched {
            Some(_) => {
                gitref::resolve_ref(repo, &gitref::tracking_ref_name(remote, SESSIONS_REF)?)?
            }
            None => None,
        };
        let tree_base = tracking_commit.clone();
        let commit_parent = tracking_commit.clone().or(old_local.clone());

        let mut changes = build_session_changes(db, repo, key, &sessions)?;

        // Carry forward already-stored, in-scope session artifacts that live
        // only in the local ref so no stored in-scope session is silently
        // dropped from the outgoing tree:
        //
        // - Remote present: a remote rewind can leave sessions in the local ref
        //   that are absent from the fetched remote tree.
        // - No remote store: the tree base is empty, so every in-scope local-only
        //   session must be re-added from the local ref.
        //
        // Passing an empty tracking set in the no-remote case makes every local
        // session a candidate; `carry_forward_local_sessions` still re-adds only
        // in-scope ids, so out-of-scope artifacts are never carried forward.
        if let Some(local_commit) = &old_local {
            let in_scope = db.get_session_ids_for_repo(repo)?;
            let carry_tracking: &[TreeEntry] = if tracking_commit.is_some() {
                &tracking_entries
            } else {
                &[]
            };
            carry_forward_local_sessions(
                repo,
                local_commit,
                carry_tracking,
                &in_scope,
                &mut changes,
            )?;
        }

        add_meta_changes(db, repo, tree_base.as_deref(), salt, machine, &mut changes)?;

        let tree = gitref::build_tree(repo, tree_base.as_deref(), &changes)?;
        let message = format!("lore: sync {} session(s)", sessions.len());
        let commit = gitref::commit_tree(repo, &tree, commit_parent.as_deref(), &message)?;

        // CAS-update the local ref, guarding against a concurrent local sync.
        match gitref::update_ref_checked(repo, SESSIONS_REF, &commit, old_local.as_deref()) {
            Ok(()) => {}
            Err(SyncError::RefCasMismatch(_)) if !is_last => continue,
            Err(e) => return Err(e.into()),
        }

        // PUSH to the remote. A non-fast-forward means the remote moved since we
        // fetched; re-fetch and retry.
        match gitref::push(repo, remote, SESSIONS_REF) {
            Ok(()) => {}
            Err(e) if !is_last && is_non_fast_forward(&e) => continue,
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("Failed to push the lore store to '{remote}'"));
            }
        }

        let ids: Vec<Uuid> = sessions.iter().map(|s| s.id).collect();
        db.mark_sessions_synced(&ids, Utc::now())?;

        return Ok(SyncSummary {
            pulled: pulled_total,
            pushed: sessions.len(),
        });
    }

    bail!("Sync did not converge after {MAX_SYNC_ATTEMPTS} attempts due to concurrent updates")
}

/// Merges every encrypted session in `entries` into the database.
///
/// Each record is applied atomically by [`Database::merge_remote_record`]: the
/// session row and messages follow newer-wins (by message_count then ended_at)
/// while links, tags, and annotations are always merged (additive, idempotent by
/// id) so a remote addition to an already-synced session is not lost, and the
/// summary is kept only when strictly newer. Returns the number of sessions
/// whose row was imported or updated (the newer-wins branch ran).
///
/// A blob that cannot be decrypted is normally skipped (corruption or a single
/// stray entry). But if the store held session blobs and NONE of them decrypted,
/// the stored key is wrong for this store, so this returns an error rather than
/// letting the caller push locally re-encrypted sessions under the wrong key and
/// mark them synced (which would poison the store).
fn merge_remote(
    db: &mut Database,
    repo: &Path,
    entries: &[TreeEntry],
    key: &[u8],
) -> Result<usize> {
    let mut pulled = 0;
    let mut session_blobs = 0;
    let mut decrypted = 0;

    for entry in entries {
        if !is_session_blob(&entry.path) {
            continue;
        }
        session_blobs += 1;

        let blob = gitref::read_blob(repo, &entry.sha)?;
        let record = match decrypt_session_record(&blob, key) {
            Ok(record) => record,
            Err(e) => {
                // A blob we cannot decrypt (corrupt or a key mismatch on a
                // single entry) must not abort the whole merge on its own; the
                // wrong-key check below covers the all-failed case.
                tracing::debug!("Skipping undecryptable session blob {}: {e}", entry.path);
                continue;
            }
        };
        decrypted += 1;

        let imported = db.merge_remote_record(
            &record.session,
            &record.messages,
            &record.links,
            &record.tags,
            &record.annotations,
            record.summary.as_ref(),
            Utc::now(),
        )?;
        if imported {
            pulled += 1;
        }
    }

    // If the store had sessions but not a single one decrypted, the key is wrong.
    // Bail before the caller builds, pushes, and marks anything synced.
    if session_blobs > 0 && decrypted == 0 {
        bail!(
            "Could not decrypt any of the {session_blobs} session(s) in the remote lore store. \
             The sync key stored on this machine does not match this store's passphrase. \
             Run 'lore sync setup' to re-enter the correct passphrase."
        );
    }

    Ok(pulled)
}

/// Merges the remote machine registry into the local database.
fn merge_machines(db: &Database, repo: &Path, entries: &[TreeEntry]) -> Result<()> {
    if let Some(bytes) = blob_at_path(repo, entries, "meta/machines.json")? {
        let machines: BTreeMap<String, String> = serde_json::from_slice(&bytes).unwrap_or_default();
        let now = Utc::now().to_rfc3339();
        for (id, name) in machines {
            db.upsert_machine(&Machine {
                id,
                name,
                created_at: now.clone(),
            })?;
        }
    }
    Ok(())
}

/// Builds the tree changes for the unsynced sessions.
///
/// Each session is assembled into its full record, encrypted, and written as
/// `sessions/<uuid>.enc` alongside a plaintext `sessions/<uuid>.meta.json`.
/// Already-synced sessions are untouched, preserving their existing git objects.
fn build_session_changes(
    db: &Database,
    repo: &Path,
    key: &[u8],
    sessions: &[Session],
) -> Result<BTreeMap<String, String>> {
    let mut changes = BTreeMap::new();

    for session in sessions {
        let record = assemble_record(db, session)?;
        let blob = encrypt_session_record(&record, key)?;
        let enc_sha = gitref::write_blob(repo, &blob)?;

        let meta = SessionMeta {
            id: session.id,
            tool: session.tool.clone(),
            started_at: session.started_at,
            ended_at: session.ended_at,
            message_count: session.message_count,
            machine_id: session.machine_id.clone(),
            git_branch: session.git_branch.clone(),
        };
        let meta_sha = gitref::write_blob(repo, &serde_json::to_vec(&meta)?)?;

        changes.insert(format!("sessions/{}.enc", session.id), enc_sha);
        changes.insert(format!("sessions/{}.meta.json", session.id), meta_sha);
    }

    Ok(changes)
}

/// Adds the `meta/salt` and `meta/machines.json` entries to the tree changes.
fn add_meta_changes(
    db: &Database,
    repo: &Path,
    base: Option<&str>,
    salt: &[u8],
    machine: &MachineIdentity,
    changes: &mut BTreeMap<String, String>,
) -> Result<()> {
    changes.insert("meta/salt".to_string(), gitref::write_blob(repo, salt)?);

    let mut machines = match base {
        Some(reference) => read_machines(repo, reference)?,
        None => BTreeMap::new(),
    };
    for m in db.list_machines()? {
        machines.insert(m.id, m.name);
    }
    machines.insert(machine.id.clone(), machine.name.clone());

    changes.insert(
        "meta/machines.json".to_string(),
        gitref::write_blob(repo, &serde_json::to_vec(&machines)?)?,
    );
    Ok(())
}

/// Assembles the complete reasoning record for a session from the database.
fn assemble_record(db: &Database, session: &Session) -> Result<SessionRecord> {
    Ok(SessionRecord {
        session: session.clone(),
        messages: db.get_messages(&session.id)?,
        links: db.get_links_by_session(&session.id)?,
        tags: db.get_tags(&session.id)?,
        annotations: db.get_annotations(&session.id)?,
        summary: db.get_summary(&session.id)?,
    })
}

// ==================== status ====================

/// Shows sync status for the current repository's lore store.
fn run_status(remote: &str, format: OutputFormat) -> Result<()> {
    let repo = current_repo()?;
    let config = Config::load()?;
    let keystore = KeyStore::with_keychain(config.use_keychain);
    let db = Database::open_default()?;

    let salt = read_store_salt(&repo, remote)?;
    let keyed = match &salt {
        Some(salt) => keystore.load_key(&store_id_from_salt(salt))?.is_some(),
        None => false,
    };
    let set_up = salt.is_some();

    // Scope the pending count to this repo so it reflects what a sync will push.
    let unsynced = db.unsynced_session_count_for_repo(&repo)?;
    let last_sync = db.last_sync_time()?;
    let remote_exists = gitref::remote_ref_exists(&repo, remote, SESSIONS_REF).unwrap_or(false);
    let local_ref = gitref::resolve_ref(&repo, SESSIONS_REF)?;
    let tracking_ref =
        gitref::resolve_ref(&repo, &gitref::tracking_ref_name(remote, SESSIONS_REF)?)?;

    match format {
        OutputFormat::Json => {
            let output = StatusOutput {
                set_up,
                keyed,
                unsynced_sessions: unsynced,
                last_sync_at: last_sync.map(|t| t.to_rfc3339()),
                remote_store_exists: remote_exists,
                local_ref: local_ref.clone(),
                tracking_ref: tracking_ref.clone(),
                remote: remote.to_string(),
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!("{}", "Lore Sync".bold());
            println!();
            if set_up && keyed {
                println!("  Store:          {}", "set up".green());
            } else if set_up {
                println!(
                    "  Store:          {} (run 'lore sync setup')",
                    "no key on this machine".yellow()
                );
            } else {
                println!(
                    "  Store:          {} (run 'lore sync setup')",
                    "not set up".yellow()
                );
            }
            println!("  Remote:         {remote}");
            println!(
                "  Remote store:   {}",
                if remote_exists { "present" } else { "none" }
            );
            println!("  Pending sync:   {unsynced}");
            match last_sync {
                Some(t) => println!("  Last sync:      {}", t.to_rfc3339()),
                None => println!("  Last sync:      {}", "never".dimmed()),
            }
            println!(
                "  Local ref:      {}",
                local_ref.as_deref().unwrap_or("none")
            );
            println!(
                "  Tracking ref:   {}",
                tracking_ref.as_deref().unwrap_or("none")
            );
        }
    }

    Ok(())
}

// ==================== helpers ====================

/// Loads the store's key and salt, or errors pointing the user to setup.
fn load_store_credentials(
    repo: &Path,
    remote: &str,
    keystore: &KeyStore,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let salt = read_store_salt(repo, remote)?.ok_or_else(|| {
        anyhow!("This repo's lore store is not set up. Run 'lore sync setup' first.")
    })?;
    let store_id = store_id_from_salt(&salt);
    let key = keystore.load_key(&store_id)?.ok_or_else(|| {
        anyhow!("No sync key for this repo on this machine. Run 'lore sync setup' first.")
    })?;
    Ok((key, salt))
}

/// Reads the store salt from the local ref, falling back to the tracking ref.
fn read_store_salt(repo: &Path, remote: &str) -> Result<Option<Vec<u8>>> {
    if gitref::ref_exists(repo, SESSIONS_REF)? {
        if let Some(bytes) = read_ref_path(repo, SESSIONS_REF, "meta/salt")? {
            return Ok(Some(bytes));
        }
    }
    let tracking = gitref::tracking_ref_name(remote, SESSIONS_REF)?;
    if gitref::ref_exists(repo, &tracking)? {
        if let Some(bytes) = read_ref_path(repo, &tracking, "meta/salt")? {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

/// Returns the bytes of the first encrypted session in the store, if any.
///
/// Prefers the remote-tracking ref (freshly fetched) and falls back to the
/// local ref, so a join can verify the passphrase against real ciphertext.
fn first_session_blob(repo: &Path, remote: &str) -> Result<Option<Vec<u8>>> {
    let tracking = gitref::tracking_ref_name(remote, SESSIONS_REF)?;
    for reference in [tracking.as_str(), SESSIONS_REF] {
        if !gitref::ref_exists(repo, reference)? {
            continue;
        }
        let entries = gitref::read_tree(repo, reference)?;
        if let Some(sha) = entries
            .iter()
            .find(|e| is_session_blob(&e.path))
            .map(|e| e.sha.clone())
        {
            return Ok(Some(gitref::read_blob(repo, &sha)?));
        }
    }
    Ok(None)
}

/// Selects the base tree and parent commit for a store-metadata write.
///
/// Prefers the remote-tracking ref (a join adopts remote state), then the local
/// ref, then none (a fresh store).
fn store_base(repo: &Path, remote: &str) -> Result<(Option<String>, Option<String>)> {
    let tracking = gitref::tracking_ref_name(remote, SESSIONS_REF)?;
    if gitref::ref_exists(repo, &tracking)? {
        return Ok((
            Some(tracking.clone()),
            gitref::resolve_ref(repo, &tracking)?,
        ));
    }
    if let Some(commit) = gitref::resolve_ref(repo, SESSIONS_REF)? {
        return Ok((Some(SESSIONS_REF.to_string()), Some(commit)));
    }
    Ok((None, None))
}

/// Reads and parses `meta/machines.json` from a ref or tree-ish.
fn read_machines(repo: &Path, reference: &str) -> Result<BTreeMap<String, String>> {
    match read_ref_path(repo, reference, "meta/machines.json")? {
        Some(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        None => Ok(BTreeMap::new()),
    }
}

/// Reads a single path's blob bytes from a ref or tree-ish.
fn read_ref_path(repo: &Path, reference: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let entries = gitref::read_tree(repo, reference)?;
    blob_at_path(repo, &entries, path)
}

/// Reads a single path's blob bytes from an already-read tree.
fn blob_at_path(repo: &Path, entries: &[TreeEntry], path: &str) -> Result<Option<Vec<u8>>> {
    match entries.iter().find(|e| e.path == path) {
        Some(entry) => Ok(Some(gitref::read_blob(repo, &entry.sha)?)),
        None => Ok(None),
    }
}

/// Returns whether a tree path is an encrypted session blob.
fn is_session_blob(path: &str) -> bool {
    path.starts_with("sessions/") && path.ends_with(".enc")
}

/// Returns whether a tree path is a stored session artifact.
///
/// Covers both the encrypted record (`sessions/<id>.enc`) and its plaintext
/// metadata sidecar (`sessions/<id>.meta.json`), the two entries a stored
/// session contributes to the tree.
fn is_session_path(path: &str) -> bool {
    path.starts_with("sessions/") && (path.ends_with(".enc") || path.ends_with(".meta.json"))
}

/// Parses the session UUID from a stored session artifact tree path.
///
/// Handles both artifact forms a session contributes to the tree:
/// `sessions/<uuid>.enc` and `sessions/<uuid>.meta.json`. Returns `None` for any
/// path that is not a recognizable session artifact or whose stem is not a
/// UUID, so callers can conservatively skip it.
fn session_uuid_from_path(path: &str) -> Option<Uuid> {
    let name = path.strip_prefix("sessions/")?;
    let stem = name
        .strip_suffix(".meta.json")
        .or_else(|| name.strip_suffix(".enc"))?;
    Uuid::parse_str(stem).ok()
}

/// Carries forward in-scope, local-only session artifacts when rebasing on the
/// remote.
///
/// When the outgoing tree is based on the remote tracking commit, any session
/// entry present in the local ref but absent from the fetched remote tree (a
/// remote rewind or reset, and whose session is already synced so it is not in
/// the freshly re-encrypted set) would be dropped. This re-adds each such entry
/// at its existing blob SHA. A freshly re-encrypted unsynced session already
/// owns its paths in `changes`, so `or_insert` never overwrites those.
///
/// Only artifacts whose session id is in `in_scope` (the ids of this repo's
/// sessions, from [`Database::get_session_ids_for_repo`]) are carried forward.
/// A local ref can hold out-of-scope sessions when it was written before per-repo
/// scoping existed; carrying those forward would re-push another repo's history
/// and reopen the privacy leak, so any artifact that is out of scope, or whose id
/// cannot be parsed or is not in the local database, is conservatively skipped.
fn carry_forward_local_sessions(
    repo: &Path,
    local_commit: &str,
    tracking_entries: &[TreeEntry],
    in_scope: &HashSet<Uuid>,
    changes: &mut BTreeMap<String, String>,
) -> Result<()> {
    let tracking_paths: HashSet<&str> = tracking_entries.iter().map(|e| e.path.as_str()).collect();

    for entry in gitref::read_tree(repo, local_commit)? {
        if !is_session_path(&entry.path) || tracking_paths.contains(entry.path.as_str()) {
            continue;
        }
        match session_uuid_from_path(&entry.path) {
            Some(id) if in_scope.contains(&id) => {}
            _ => continue,
        }
        changes.entry(entry.path.clone()).or_insert(entry.sha);
    }
    Ok(())
}

/// Heuristically detects a non-fast-forward push rejection for retry.
///
/// Push rejection wording is not localized by git for the machine-readable
/// markers checked here, but this is only used to decide whether to re-fetch and
/// retry; the final attempt surfaces the original error regardless.
fn is_non_fast_forward(err: &SyncError) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("fast-forward")
        || message.contains("non-fast")
        || message.contains("rejected")
        || message.contains("fetch first")
        || message.contains("stale info")
}

/// Resolves the current git repository's top-level directory.
fn current_repo() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to run git")?;

    if !output.status.success() {
        bail!("Not inside a git repository. Run this command from within a repo.");
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        bail!("Not inside a git repository. Run this command from within a repo.");
    }
    Ok(PathBuf::from(path))
}

/// Builds this machine's identity from config, persisting a generated id.
fn machine_identity(config: &mut Config) -> Result<MachineIdentity> {
    let id = config.get_or_create_machine_id()?;
    let name = config.get_machine_name();
    Ok(MachineIdentity { id, name })
}

/// Prompts for a new passphrase with confirmation and a minimum length.
fn prompt_new_passphrase() -> Result<String> {
    loop {
        print!("Enter passphrase: ");
        io::stdout().flush()?;
        let passphrase = rpassword::read_password()?;

        if passphrase.len() < MIN_PASSPHRASE_LEN {
            println!(
                "{}",
                format!("Passphrase must be at least {MIN_PASSPHRASE_LEN} characters.").red()
            );
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

/// Prompts for an existing passphrase without confirmation.
fn prompt_passphrase() -> Result<String> {
    print!("Passphrase: ");
    io::stdout().flush()?;
    let passphrase = rpassword::read_password()?;
    Ok(passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{
        Annotation, LinkCreator, LinkType, Message, MessageContent, MessageRole, SessionLink,
        Summary, Tag,
    };
    use std::process::Command;
    use tempfile::TempDir;

    /// Runs a git command in a test repo, asserting success.
    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("failed to spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Runs a git command in a test repo, asserting success and returning stdout.
    fn git_out(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("failed to spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Initializes a temp repo with an identity and signing disabled.
    fn init_repo(repo: &Path) {
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.name", "Lore Test"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        git(repo, &["config", "commit.gpgsign", "false"]);
        git(repo, &["config", "tag.gpgsign", "false"]);
    }

    /// Creates a bare remote repo and returns its temp dir and URL string.
    fn init_bare_remote() -> (TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--bare", "-q"]);
        let url = dir.path().to_str().unwrap().to_string();
        (dir, url)
    }

    /// Opens a fresh database in a temp directory.
    fn open_db() -> (Database, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("lore.db")).unwrap();
        (db, dir)
    }

    /// Builds a file-backed key store isolated to a temp directory.
    fn test_keystore() -> (KeyStore, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::with_base_dir(dir.path().to_path_buf(), false);
        (store, dir)
    }

    fn machine(id: &str, name: &str) -> MachineIdentity {
        MachineIdentity {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    /// Returns the canonical string form of a repo path.
    ///
    /// A session must be seeded with the same path the scoped selector derives
    /// (which canonicalizes the repo, resolving symlinks such as macOS's
    /// `/var` -> `/private/var`) so it stays in scope and is actually pushed.
    fn repo_dir(repo: &Path) -> String {
        repo.canonicalize().unwrap().to_string_lossy().to_string()
    }

    /// Returns this repo's unsynced sessions, the set a real sync would push.
    fn scoped_unsynced(db: &Database, repo: &Path) -> Vec<Session> {
        db.get_unsynced_sessions_for_repo(repo).unwrap()
    }

    /// Seeds a full unsynced session (messages, link, tag, annotation, summary).
    ///
    /// `working_directory` must be inside the repo under test for the session to
    /// be selected by the repo-scoped sync.
    fn seed_full_session(db: &mut Database, machine_id: &str, working_directory: &str) -> Uuid {
        let id = Uuid::new_v4();
        let session = Session {
            id,
            tool: "claude-code".to_string(),
            tool_version: Some("1.0.0".to_string()),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            model: Some("claude-opus".to_string()),
            working_directory: working_directory.to_string(),
            git_branch: Some("main".to_string()),
            source_path: None,
            message_count: 1,
            machine_id: Some(machine_id.to_string()),
        };
        let message = Message {
            id: Uuid::new_v4(),
            session_id: id,
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::User,
            content: MessageContent::Text("fix the bug".to_string()),
            model: None,
            git_branch: Some("main".to_string()),
            cwd: Some(working_directory.to_string()),
        };
        // synced_at = None so the session is picked up by get_unsynced_sessions.
        db.import_session_with_messages(&session, &[message], None)
            .unwrap();
        db.insert_link(&SessionLink {
            id: Uuid::new_v4(),
            session_id: id,
            link_type: LinkType::Commit,
            commit_sha: Some("deadbeef".to_string()),
            branch: Some("main".to_string()),
            remote: Some("origin".to_string()),
            created_at: Utc::now(),
            created_by: LinkCreator::User,
            confidence: Some(0.9),
        })
        .unwrap();
        db.insert_tag(&Tag {
            id: Uuid::new_v4(),
            session_id: id,
            label: "feature".to_string(),
            created_at: Utc::now(),
        })
        .unwrap();
        db.insert_annotation(&Annotation {
            id: Uuid::new_v4(),
            session_id: id,
            content: "an important note".to_string(),
            created_at: Utc::now(),
        })
        .unwrap();
        db.insert_summary(&Summary {
            id: Uuid::new_v4(),
            session_id: id,
            content: "fixed the parser".to_string(),
            generated_at: Utc::now(),
        })
        .unwrap();
        id
    }

    /// Returns the blob SHA of the single session `.enc` entry in the local ref.
    fn session_blob_sha(repo: &Path) -> String {
        let entries = gitref::read_tree(repo, SESSIONS_REF).unwrap();
        entries
            .iter()
            .find(|e| is_session_blob(&e.path))
            .expect("a session blob should exist")
            .sha
            .clone()
    }

    /// Returns the tree path of the single session `.enc` entry in the local ref.
    fn session_blob_sha_path(repo: &Path) -> String {
        let entries = gitref::read_tree(repo, SESSIONS_REF).unwrap();
        entries
            .iter()
            .find(|e| is_session_blob(&e.path))
            .expect("a session blob should exist")
            .path
            .clone()
    }

    #[test]
    fn test_create_store_writes_salt_and_pushes() {
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");

        create_store(repo, "origin", &keystore, &m, "correct horse battery").unwrap();

        // The local ref exists and holds the salt.
        assert!(gitref::ref_exists(repo, SESSIONS_REF).unwrap());
        let salt = read_store_salt(repo, "origin")
            .unwrap()
            .expect("salt written");
        // The derived key is stored under the salt-derived store id.
        let store_id = store_id_from_salt(&salt);
        assert!(keystore.load_key(&store_id).unwrap().is_some());
        // The store was pushed to the remote.
        assert!(gitref::remote_ref_exists(repo, "origin", SESSIONS_REF).unwrap());
    }

    #[test]
    fn test_sync_round_trip_between_machines() {
        let (_remote_dir, remote_url) = init_bare_remote();
        let passphrase = "shared team passphrase";

        // Machine A: set up, seed a full session, and sync (push).
        let dir_a = tempfile::tempdir().unwrap();
        let repo_a = dir_a.path();
        init_repo(repo_a);
        git(repo_a, &["remote", "add", "origin", &remote_url]);
        let (keystore_a, _ka) = test_keystore();
        let ma = machine("machine-a", "Machine A");
        create_store(repo_a, "origin", &keystore_a, &ma, passphrase).unwrap();

        let (mut db_a, _da) = open_db();
        let session_id = seed_full_session(&mut db_a, "machine-a", &repo_dir(repo_a));
        let (key_a, salt_a) = load_store_credentials(repo_a, "origin", &keystore_a).unwrap();
        let sessions_a = scoped_unsynced(&db_a, repo_a);
        let summary_a = perform_sync(
            &mut db_a, repo_a, "origin", &key_a, &salt_a, &ma, sessions_a,
        )
        .unwrap();
        assert_eq!(summary_a.pushed, 1);

        // Machine B: join with the same passphrase, then sync (pull).
        let dir_b = tempfile::tempdir().unwrap();
        let repo_b = dir_b.path();
        init_repo(repo_b);
        git(repo_b, &["remote", "add", "origin", &remote_url]);
        let (keystore_b, _kb) = test_keystore();
        let mb = machine("machine-b", "Machine B");
        gitref::fetch(repo_b, "origin", SESSIONS_REF).unwrap();
        let salt_b = read_store_salt(repo_b, "origin").unwrap().unwrap();
        join_store(repo_b, "origin", &keystore_b, &mb, &salt_b, passphrase).unwrap();

        let (mut db_b, _db) = open_db();
        let (key_b, salt_b2) = load_store_credentials(repo_b, "origin", &keystore_b).unwrap();
        // Both machines derive the same key from the shared passphrase and salt.
        assert_eq!(key_a, key_b);
        let sessions_b = scoped_unsynced(&db_b, repo_b);
        let summary_b = perform_sync(
            &mut db_b, repo_b, "origin", &key_b, &salt_b2, &mb, sessions_b,
        )
        .unwrap();
        assert_eq!(summary_b.pulled, 1);

        // Machine B now has the full reasoning record, including links, tags,
        // and the summary.
        let pulled = db_b
            .get_session(&session_id)
            .unwrap()
            .expect("session pulled");
        assert_eq!(pulled.tool, "claude-code");
        assert_eq!(db_b.get_messages(&session_id).unwrap().len(), 1);
        let links = db_b.get_links_by_session(&session_id).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].commit_sha, Some("deadbeef".to_string()));
        assert_eq!(db_b.get_tags(&session_id).unwrap()[0].label, "feature");
        assert_eq!(db_b.get_annotations(&session_id).unwrap().len(), 1);
        assert_eq!(
            db_b.get_summary(&session_id).unwrap().unwrap().content,
            "fixed the parser"
        );
    }

    #[test]
    fn test_incremental_sync_preserves_unchanged_blob() {
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &keystore, &m, "passphrase one two").unwrap();

        let (mut db, _dd) = open_db();
        seed_full_session(&mut db, "machine-a", &repo_dir(repo));
        let (key, salt) = load_store_credentials(repo, "origin", &keystore).unwrap();

        let sessions = scoped_unsynced(&db, repo);
        perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();
        let sha_first = session_blob_sha(repo);

        // A second sync with nothing new must not re-encrypt the session, so its
        // content-addressed blob object stays byte-identical (near-zero growth).
        let sessions = scoped_unsynced(&db, repo);
        let summary = perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();
        assert_eq!(summary.pushed, 0);
        let sha_second = session_blob_sha(repo);
        assert_eq!(
            sha_first, sha_second,
            "unchanged session blob must be reused"
        );
    }

    #[test]
    fn test_sync_errors_when_not_set_up() {
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let err = load_store_credentials(repo, "origin", &keystore).unwrap_err();
        assert!(
            err.to_string().contains("lore sync setup"),
            "error should point to setup: {err}"
        );
    }

    #[test]
    fn test_join_with_wrong_passphrase_fails() {
        let (_remote_dir, remote_url) = init_bare_remote();

        // Machine A creates the store and pushes a real session.
        let dir_a = tempfile::tempdir().unwrap();
        let repo_a = dir_a.path();
        init_repo(repo_a);
        git(repo_a, &["remote", "add", "origin", &remote_url]);
        let (keystore_a, _ka) = test_keystore();
        let ma = machine("machine-a", "Machine A");
        create_store(repo_a, "origin", &keystore_a, &ma, "the real passphrase").unwrap();
        let (mut db_a, _da) = open_db();
        seed_full_session(&mut db_a, "machine-a", &repo_dir(repo_a));
        let (key_a, salt_a) = load_store_credentials(repo_a, "origin", &keystore_a).unwrap();
        let sessions_a = scoped_unsynced(&db_a, repo_a);
        perform_sync(
            &mut db_a, repo_a, "origin", &key_a, &salt_a, &ma, sessions_a,
        )
        .unwrap();

        // Machine B tries to join with the wrong passphrase.
        let dir_b = tempfile::tempdir().unwrap();
        let repo_b = dir_b.path();
        init_repo(repo_b);
        git(repo_b, &["remote", "add", "origin", &remote_url]);
        let (keystore_b, _kb) = test_keystore();
        let mb = machine("machine-b", "Machine B");
        gitref::fetch(repo_b, "origin", SESSIONS_REF).unwrap();
        let salt_b = read_store_salt(repo_b, "origin").unwrap().unwrap();

        let err = join_store(
            repo_b,
            "origin",
            &keystore_b,
            &mb,
            &salt_b,
            "the WRONG passphrase",
        )
        .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("passphrase"),
            "wrong passphrase should be reported: {err}"
        );
    }

    #[test]
    fn test_merge_skips_older_remote_session() {
        // A remote record that is not newer than the local copy is left as-is.
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);
        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &keystore, &m, "passphrase abcdefgh").unwrap();
        let (key, _salt) = load_store_credentials(repo, "origin", &keystore).unwrap();

        // Local session with two messages.
        let (mut db, _dd) = open_db();
        let id = Uuid::new_v4();
        let mut session = Session {
            id,
            tool: "claude-code".to_string(),
            tool_version: None,
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            model: None,
            working_directory: "/proj".to_string(),
            git_branch: None,
            source_path: None,
            message_count: 2,
            machine_id: Some("machine-a".to_string()),
        };
        db.import_session_with_messages(&session, &[], Some(Utc::now()))
            .unwrap();

        // Build an older remote record (fewer messages) and encrypt it as a blob.
        session.message_count = 1;
        let record = SessionRecord {
            session: session.clone(),
            messages: vec![],
            links: vec![],
            tags: vec![],
            annotations: vec![],
            summary: None,
        };
        let blob = encrypt_session_record(&record, &key).unwrap();
        let sha = gitref::write_blob(repo, &blob).unwrap();
        let entries = vec![TreeEntry {
            mode: "100644".to_string(),
            sha,
            path: format!("sessions/{id}.enc"),
        }];

        let pulled = merge_remote(&mut db, repo, &entries, &key).unwrap();
        assert_eq!(pulled, 0, "older remote session must be skipped");
        assert_eq!(db.get_session(&id).unwrap().unwrap().message_count, 2);
    }

    #[test]
    fn test_is_session_blob() {
        assert!(is_session_blob("sessions/abc.enc"));
        assert!(!is_session_blob("sessions/abc.meta.json"));
        assert!(!is_session_blob("meta/salt"));
        assert!(!is_session_blob("abc.enc"));
    }

    #[test]
    fn test_is_non_fast_forward_detects_rejection() {
        let rejected = SyncError::Git(
            "git push origin failed: ! [rejected] refs/lore/sessions (fetch first)".to_string(),
        );
        assert!(is_non_fast_forward(&rejected));
        let unrelated = SyncError::Git("git push origin failed: permission denied".to_string());
        assert!(!is_non_fast_forward(&unrelated));
    }

    #[test]
    fn test_is_session_path() {
        assert!(is_session_path("sessions/abc.enc"));
        assert!(is_session_path("sessions/abc.meta.json"));
        assert!(!is_session_path("meta/salt"));
        assert!(!is_session_path("abc.enc"));
    }

    #[test]
    fn test_added_link_reopens_session_and_syncs() {
        // Adding a link to an already-synced session must re-export it, so the
        // new link rides along to a teammate on the next sync.
        let (_remote_dir, remote_url) = init_bare_remote();
        let passphrase = "shared team passphrase";

        // Machine A: set up, seed a session, and sync it (push).
        let dir_a = tempfile::tempdir().unwrap();
        let repo_a = dir_a.path();
        init_repo(repo_a);
        git(repo_a, &["remote", "add", "origin", &remote_url]);
        let (keystore_a, _ka) = test_keystore();
        let ma = machine("machine-a", "Machine A");
        create_store(repo_a, "origin", &keystore_a, &ma, passphrase).unwrap();

        let (mut db_a, _da) = open_db();
        let session_id = seed_full_session(&mut db_a, "machine-a", &repo_dir(repo_a));
        let (key_a, salt_a) = load_store_credentials(repo_a, "origin", &keystore_a).unwrap();
        let sessions_a = scoped_unsynced(&db_a, repo_a);
        let first = perform_sync(
            &mut db_a, repo_a, "origin", &key_a, &salt_a, &ma, sessions_a,
        )
        .unwrap();
        assert_eq!(first.pushed, 1);

        // The session is now synced; adding a link locally must re-open it.
        db_a.insert_link(&SessionLink {
            id: Uuid::new_v4(),
            session_id,
            link_type: LinkType::Commit,
            commit_sha: Some("feedface".to_string()),
            branch: Some("main".to_string()),
            remote: Some("origin".to_string()),
            created_at: Utc::now(),
            created_by: LinkCreator::Auto,
            confidence: Some(0.8),
        })
        .unwrap();

        let sessions_a = scoped_unsynced(&db_a, repo_a);
        let second = perform_sync(
            &mut db_a, repo_a, "origin", &key_a, &salt_a, &ma, sessions_a,
        )
        .unwrap();
        assert_eq!(
            second.pushed, 1,
            "adding a link must re-export the parent session"
        );

        // Machine B joins and pulls: it must see both links.
        let dir_b = tempfile::tempdir().unwrap();
        let repo_b = dir_b.path();
        init_repo(repo_b);
        git(repo_b, &["remote", "add", "origin", &remote_url]);
        let (keystore_b, _kb) = test_keystore();
        let mb = machine("machine-b", "Machine B");
        gitref::fetch(repo_b, "origin", SESSIONS_REF).unwrap();
        let salt_b = read_store_salt(repo_b, "origin").unwrap().unwrap();
        join_store(repo_b, "origin", &keystore_b, &mb, &salt_b, passphrase).unwrap();
        let (mut db_b, _db) = open_db();
        let (key_b, salt_b2) = load_store_credentials(repo_b, "origin", &keystore_b).unwrap();
        let sessions_b = scoped_unsynced(&db_b, repo_b);
        perform_sync(
            &mut db_b, repo_b, "origin", &key_b, &salt_b2, &mb, sessions_b,
        )
        .unwrap();

        let links = db_b.get_links_by_session(&session_id).unwrap();
        assert_eq!(links.len(), 2, "both links must reach the teammate");
    }

    #[test]
    fn test_wrong_key_errors_before_push() {
        // A sync whose stored key does not match a non-empty remote store must
        // error before pushing, and must not mark local sessions synced.
        let (_remote_dir, remote_url) = init_bare_remote();

        // Machine A creates the store and pushes a real (correctly keyed) session.
        let dir_a = tempfile::tempdir().unwrap();
        let repo_a = dir_a.path();
        init_repo(repo_a);
        git(repo_a, &["remote", "add", "origin", &remote_url]);
        let (keystore_a, _ka) = test_keystore();
        let ma = machine("machine-a", "Machine A");
        create_store(repo_a, "origin", &keystore_a, &ma, "the real passphrase").unwrap();
        let (mut db_a, _da) = open_db();
        seed_full_session(&mut db_a, "machine-a", &repo_dir(repo_a));
        let (key_a, salt_a) = load_store_credentials(repo_a, "origin", &keystore_a).unwrap();
        let sessions_a = scoped_unsynced(&db_a, repo_a);
        perform_sync(
            &mut db_a, repo_a, "origin", &key_a, &salt_a, &ma, sessions_a,
        )
        .unwrap();

        // Machine B stores a WRONG key for the same store (same salt, bad pass).
        let dir_b = tempfile::tempdir().unwrap();
        let repo_b = dir_b.path();
        init_repo(repo_b);
        git(repo_b, &["remote", "add", "origin", &remote_url]);
        let (keystore_b, _kb) = test_keystore();
        let mb = machine("machine-b", "Machine B");
        gitref::fetch(repo_b, "origin", SESSIONS_REF).unwrap();
        let salt_b = read_store_salt(repo_b, "origin").unwrap().unwrap();
        let wrong_key = derive_store_key("the WRONG passphrase", &salt_b).unwrap();
        keystore_b
            .store_key(&store_id_from_salt(&salt_b), &wrong_key)
            .unwrap();

        // Machine B has a local unsynced session that must not be pushed/marked.
        let (mut db_b, _db) = open_db();
        let local_id = seed_full_session(&mut db_b, "machine-b", &repo_dir(repo_b));
        let sessions_b = scoped_unsynced(&db_b, repo_b);

        let err = perform_sync(
            &mut db_b, repo_b, "origin", &wrong_key, &salt_b, &mb, sessions_b,
        )
        .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("passphrase") || msg.contains("decrypt"),
            "wrong key must be reported clearly: {err}"
        );

        // The local session must remain unsynced (nothing was pushed).
        let unsynced = db_b.get_unsynced_sessions().unwrap();
        assert!(
            unsynced.iter().any(|s| s.id == local_id),
            "local session must not be marked synced after a wrong-key failure"
        );
    }

    #[test]
    fn test_carry_forward_local_sessions_readds_missing() {
        // A session artifact present only in the local ref (missing from the
        // fetched remote tree) must be carried into the outgoing changes.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let a_enc = format!("sessions/{id_a}.enc");
        let a_meta = format!("sessions/{id_a}.meta.json");
        let b_enc = format!("sessions/{id_b}.enc");

        let enc_sha = gitref::write_blob(repo, b"local-a-enc").unwrap();
        let meta_sha = gitref::write_blob(repo, b"local-a-meta").unwrap();
        let other_sha = gitref::write_blob(repo, b"remote-b-enc").unwrap();

        let mut local = BTreeMap::new();
        local.insert(a_enc.clone(), enc_sha.clone());
        local.insert(a_meta.clone(), meta_sha.clone());
        local.insert(b_enc.clone(), other_sha.clone());
        let local_tree = gitref::build_tree(repo, None, &local).unwrap();
        let local_commit = gitref::commit_tree(repo, &local_tree, None, "lore: local").unwrap();

        // The remote (tracking) tree only has session b.
        let tracking = vec![TreeEntry {
            mode: "100644".to_string(),
            sha: other_sha,
            path: b_enc.clone(),
        }];

        // A freshly re-encrypted unsynced session owns its own path already.
        let mut changes = BTreeMap::new();
        changes.insert(a_enc.clone(), "fresh-sha".to_string());

        // Both sessions are in scope for this repo.
        let in_scope: HashSet<Uuid> = [id_a, id_b].into_iter().collect();
        carry_forward_local_sessions(repo, &local_commit, &tracking, &in_scope, &mut changes)
            .unwrap();

        // a.meta.json (local-only) is carried forward.
        assert_eq!(changes.get(&a_meta), Some(&meta_sha));
        // a.enc keeps the fresh re-encrypted value (or_insert must not override).
        assert_eq!(changes.get(&a_enc), Some(&"fresh-sha".to_string()));
        // b.enc is in the tracking tree, so it is not re-added.
        assert!(!changes.contains_key(&b_enc));
    }

    #[test]
    fn test_carry_forward_skips_out_of_scope_sessions() {
        // A local-only session artifact whose id is NOT in this repo's scope must
        // not be carried forward. This is the privacy regression the scoped
        // carry-forward closes: a local ref written before scoping can hold other
        // repos' sessions, and re-pushing them would leak that history.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        let id_in = Uuid::new_v4();
        let id_out = Uuid::new_v4();
        let in_enc = format!("sessions/{id_in}.enc");
        let in_meta = format!("sessions/{id_in}.meta.json");
        let out_enc = format!("sessions/{id_out}.enc");
        let out_meta = format!("sessions/{id_out}.meta.json");

        let in_enc_sha = gitref::write_blob(repo, b"in-enc").unwrap();
        let in_meta_sha = gitref::write_blob(repo, b"in-meta").unwrap();
        let out_enc_sha = gitref::write_blob(repo, b"out-enc").unwrap();
        let out_meta_sha = gitref::write_blob(repo, b"out-meta").unwrap();

        let mut local = BTreeMap::new();
        local.insert(in_enc.clone(), in_enc_sha);
        local.insert(in_meta.clone(), in_meta_sha);
        local.insert(out_enc.clone(), out_enc_sha);
        local.insert(out_meta.clone(), out_meta_sha);
        let local_tree = gitref::build_tree(repo, None, &local).unwrap();
        let local_commit = gitref::commit_tree(repo, &local_tree, None, "lore: local").unwrap();

        // The remote tree is empty (a rewind), so nothing is already present.
        let tracking: Vec<TreeEntry> = Vec::new();

        // Only the in-scope session's id is in scope.
        let in_scope: HashSet<Uuid> = [id_in].into_iter().collect();
        let mut changes = BTreeMap::new();
        carry_forward_local_sessions(repo, &local_commit, &tracking, &in_scope, &mut changes)
            .unwrap();

        // In-scope artifacts are carried forward.
        assert!(changes.contains_key(&in_enc), "in-scope .enc carried");
        assert!(changes.contains_key(&in_meta), "in-scope .meta carried");
        // Out-of-scope artifacts are dropped, not re-pushed.
        assert!(
            !changes.contains_key(&out_enc),
            "out-of-scope .enc must not be carried forward"
        );
        assert!(
            !changes.contains_key(&out_meta),
            "out-of-scope .meta must not be carried forward"
        );
    }

    #[test]
    fn test_sync_quiet_noops_when_not_set_up() {
        // A repo with a remote but no lore store must be a silent no-op. It must
        // not error and must not touch the config or database (this test does
        // not open ~/.lore, so reaching that code would surface as a failure).
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        sync_quiet_in_repo(repo, "origin").expect("quiet sync must no-op when not set up");
    }

    #[test]
    fn test_sync_quiet_noops_without_remote() {
        // No configured remote at all: quiet sync skips gracefully.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        sync_quiet_in_repo(repo, "origin").expect("quiet sync must no-op without a remote");
    }

    #[test]
    fn test_quiet_sync_no_key_returns_ok_without_mutating() {
        // Set up a store (salt in the ref) but present a key store that holds no
        // key for it. The quiet path must return Ok before reaching the mutating
        // machine-id step or opening the database. The key store is isolated to a
        // temp dir, so load_key resolves to None deterministically.
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        // Create the store with one key store, then run the quiet sync against a
        // different, empty key store to model "set up elsewhere, no key here".
        let (setup_keystore, _sk) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &setup_keystore, &m, "passphrase abcdefgh").unwrap();

        let salt = read_store_salt(repo, "origin")
            .unwrap()
            .expect("salt present");
        let (empty_keystore, _ek) = test_keystore();
        assert!(
            empty_keystore
                .load_key(&store_id_from_salt(&salt))
                .unwrap()
                .is_none(),
            "the isolated key store must have no key for this store"
        );

        quiet_sync_with_keystore(repo, "origin", &salt, &empty_keystore)
            .expect("quiet sync must no-op when no key is stored on this machine");
    }

    #[test]
    fn test_resolve_hook_remote_prefers_named_remote() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(
            repo,
            &["remote", "add", "origin", "https://example.com/a.git"],
        );
        git(
            repo,
            &["remote", "add", "upstream", "https://example.com/b.git"],
        );

        assert_eq!(
            resolve_hook_remote(repo, "upstream").unwrap(),
            Some("upstream".to_string())
        );
    }

    #[test]
    fn test_resolve_hook_remote_falls_back_to_origin_for_url() {
        // git can pass a raw URL for an anonymous push; fall back to origin.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(
            repo,
            &["remote", "add", "origin", "https://example.com/a.git"],
        );

        assert_eq!(
            resolve_hook_remote(repo, "https://example.com/other.git").unwrap(),
            Some("origin".to_string())
        );
    }

    #[test]
    fn test_resolve_hook_remote_none_when_unknown_and_no_origin() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(
            repo,
            &["remote", "add", "upstream", "https://example.com/b.git"],
        );

        assert_eq!(
            resolve_hook_remote(repo, "https://example.com/x.git").unwrap(),
            None
        );
    }

    #[test]
    fn test_local_only_session_survives_remote_rewind() {
        // If the remote store rewinds to before a session, a sync that rebases on
        // the remote must still keep the already-stored local session.
        let (remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &keystore, &m, "passphrase abcdefgh").unwrap();

        // The store-setup commit (no sessions) is our rewind target.
        let setup_commit = git_out(remote_dir.path(), &["rev-parse", SESSIONS_REF]);

        let (mut db, _dd) = open_db();
        seed_full_session(&mut db, "machine-a", &repo_dir(repo));
        let (key, salt) = load_store_credentials(repo, "origin", &keystore).unwrap();
        let sessions = scoped_unsynced(&db, repo);
        perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();

        let session_enc = session_blob_sha_path(repo);

        // Simulate a remote rewind: force the remote lore ref back to setup.
        git(
            remote_dir.path(),
            &["update-ref", SESSIONS_REF, setup_commit.trim()],
        );

        // Sync again. The already-synced session is not rebuilt, so only the
        // carry-forward keeps it in the outgoing tree.
        let sessions = scoped_unsynced(&db, repo);
        perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();

        let entries = gitref::read_tree(repo, SESSIONS_REF).unwrap();
        assert!(
            entries.iter().any(|e| e.path == session_enc),
            "local-only session must survive a remote rewind"
        );
    }

    #[test]
    fn test_sync_only_pushes_in_scope_sessions() {
        // A per-repo sync must push only sessions whose working directory is
        // inside this repo. A session captured in an unrelated directory must
        // stay out of this repo's store (the privacy bug this scoping fixes) and
        // remain unsynced.
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &keystore, &m, "passphrase abcdefgh").unwrap();

        let (mut db, _dd) = open_db();
        // One session inside a subdirectory of the repo, one in an unrelated dir.
        let repo_sub = format!("{}/crate/src", repo_dir(repo));
        let in_scope = seed_full_session(&mut db, "machine-a", &repo_sub);
        let out_of_scope = seed_full_session(&mut db, "machine-a", "/somewhere/else/project");
        let (key, salt) = load_store_credentials(repo, "origin", &keystore).unwrap();

        let sessions = scoped_unsynced(&db, repo);
        let summary = perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();
        assert_eq!(
            summary.pushed, 1,
            "only the in-scope session must be pushed"
        );

        // The store holds exactly the in-scope session's encrypted blob.
        let entries = gitref::read_tree(repo, SESSIONS_REF).unwrap();
        let session_blobs = entries.iter().filter(|e| is_session_blob(&e.path)).count();
        assert_eq!(session_blobs, 1, "store must hold exactly one session blob");
        assert!(
            entries
                .iter()
                .any(|e| e.path == format!("sessions/{in_scope}.enc")),
            "the in-scope session must be stored"
        );
        assert!(
            !entries
                .iter()
                .any(|e| e.path == format!("sessions/{out_of_scope}.enc")),
            "the out-of-scope session must not reach this repo's store"
        );

        // The out-of-scope session stays unsynced; the in-scope one is marked
        // synced.
        let unsynced = db.get_unsynced_sessions().unwrap();
        let unsynced_ids: HashSet<Uuid> = unsynced.iter().map(|s| s.id).collect();
        assert!(
            unsynced_ids.contains(&out_of_scope),
            "the out-of-scope session must remain unsynced"
        );
        assert!(
            !unsynced_ids.contains(&in_scope),
            "the in-scope session must be marked synced after the push"
        );
    }

    #[test]
    fn test_sync_does_not_carry_forward_out_of_scope_local_session() {
        // A local ref written before per-repo scoping can hold another repo's
        // session artifact. When such an out-of-scope artifact lives only in the
        // local ref (absent from the remote), a sync that rebases on the remote
        // must NOT carry it forward, or it would re-push another repo's history.
        let (_remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &keystore, &m, "passphrase abcdefgh").unwrap();

        let (mut db, _dd) = open_db();
        let in_scope = seed_full_session(&mut db, "machine-a", &repo_dir(repo));
        // An out-of-scope session exists in the database (so its id is known) but
        // its working directory is outside the repo, so it is out of scope.
        let out_of_scope = seed_full_session(&mut db, "machine-a", "/somewhere/else/project");

        // Inject the out-of-scope session's artifacts into the LOCAL ref only,
        // simulating a pre-scoping local store that still holds them.
        let base = gitref::resolve_ref(repo, SESSIONS_REF).unwrap();
        let leaked_enc = gitref::write_blob(repo, b"leaked-enc").unwrap();
        let leaked_meta = gitref::write_blob(repo, b"leaked-meta").unwrap();
        let mut inject = BTreeMap::new();
        inject.insert(format!("sessions/{out_of_scope}.enc"), leaked_enc);
        inject.insert(format!("sessions/{out_of_scope}.meta.json"), leaked_meta);
        let tree = gitref::build_tree(repo, base.as_deref(), &inject).unwrap();
        let commit = gitref::commit_tree(repo, &tree, base.as_deref(), "inject leaked").unwrap();
        gitref::update_ref_checked(repo, SESSIONS_REF, &commit, base.as_deref()).unwrap();

        let (key, salt) = load_store_credentials(repo, "origin", &keystore).unwrap();
        let sessions = scoped_unsynced(&db, repo);
        perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();

        let entries = gitref::read_tree(repo, SESSIONS_REF).unwrap();
        // The in-scope session is stored.
        assert!(
            entries
                .iter()
                .any(|e| e.path == format!("sessions/{in_scope}.enc")),
            "the in-scope session must be stored"
        );
        // The out-of-scope local-only artifacts must not survive the sync.
        assert!(
            !entries
                .iter()
                .any(|e| e.path == format!("sessions/{out_of_scope}.enc")),
            "out-of-scope local-only session must not be carried forward or pushed"
        );
        assert!(
            !entries
                .iter()
                .any(|e| e.path == format!("sessions/{out_of_scope}.meta.json")),
            "out-of-scope local-only metadata must not be carried forward or pushed"
        );
    }

    #[test]
    fn test_sync_no_remote_does_not_inherit_out_of_scope_local_artifacts() {
        // No remote lore ref exists (the remote store was never initialized or was
        // deleted), so the outgoing tree base is empty rather than the local ref.
        // A local ref written before per-repo scoping can still hold out-of-scope
        // session artifacts; the no-remote path must build the tree from an empty
        // base and carry forward only in-scope local-only sessions, so those
        // out-of-scope artifacts are neither kept in the new local ref nor pushed.
        let (remote_dir, remote_url) = init_bare_remote();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", &remote_url]);

        let (keystore, _kd) = test_keystore();
        let m = machine("machine-a", "Machine A");
        create_store(repo, "origin", &keystore, &m, "passphrase abcdefgh").unwrap();

        let (mut db, _dd) = open_db();
        let in_scope = seed_full_session(&mut db, "machine-a", &repo_dir(repo));
        // An out-of-scope session exists in the database (so its id is known) but
        // its working directory is outside the repo, so it is out of scope.
        let out_of_scope = seed_full_session(&mut db, "machine-a", "/somewhere/else/project");

        // Inject the out-of-scope session's artifacts into the LOCAL ref only,
        // simulating a pre-scoping local store that still holds them.
        let base = gitref::resolve_ref(repo, SESSIONS_REF).unwrap();
        let leaked_enc = gitref::write_blob(repo, b"leaked-enc").unwrap();
        let leaked_meta = gitref::write_blob(repo, b"leaked-meta").unwrap();
        let mut inject = BTreeMap::new();
        inject.insert(format!("sessions/{out_of_scope}.enc"), leaked_enc);
        inject.insert(format!("sessions/{out_of_scope}.meta.json"), leaked_meta);
        let tree = gitref::build_tree(repo, base.as_deref(), &inject).unwrap();
        let commit = gitref::commit_tree(repo, &tree, base.as_deref(), "inject leaked").unwrap();
        gitref::update_ref_checked(repo, SESSIONS_REF, &commit, base.as_deref()).unwrap();

        // Remove the remote lore ref so the sync takes the no-remote base path
        // (fetch returns None and the tree base is empty).
        git(remote_dir.path(), &["update-ref", "-d", SESSIONS_REF]);
        assert!(!gitref::remote_ref_exists(repo, "origin", SESSIONS_REF).unwrap());

        let (key, salt) = load_store_credentials(repo, "origin", &keystore).unwrap();
        let sessions = scoped_unsynced(&db, repo);
        let summary = perform_sync(&mut db, repo, "origin", &key, &salt, &m, sessions).unwrap();
        assert_eq!(summary.pushed, 1, "only the in-scope session is pushed");

        // The remote ref is created fresh by the push.
        assert!(gitref::remote_ref_exists(repo, "origin", SESSIONS_REF).unwrap());

        // Refresh the tracking ref so the pushed remote tree can be inspected.
        gitref::fetch(repo, "origin", SESSIONS_REF).unwrap();

        // The in-scope session is present in the new local ref and on the remote,
        // while the out-of-scope local-only artifacts are gone from both.
        let tracking = gitref::tracking_ref_name("origin", SESSIONS_REF).unwrap();
        for reference in [SESSIONS_REF, tracking.as_str()] {
            let entries = gitref::read_tree(repo, reference).unwrap();
            assert!(
                entries
                    .iter()
                    .any(|e| e.path == format!("sessions/{in_scope}.enc")),
                "the in-scope session must be present in {reference}"
            );
            assert!(
                !entries
                    .iter()
                    .any(|e| e.path == format!("sessions/{out_of_scope}.enc")),
                "out-of-scope .enc must not survive the no-remote sync in {reference}"
            );
            assert!(
                !entries
                    .iter()
                    .any(|e| e.path == format!("sessions/{out_of_scope}.meta.json")),
                "out-of-scope .meta must not survive the no-remote sync in {reference}"
            );
            // The store metadata still lands in the tree in the no-remote case.
            assert!(
                entries.iter().any(|e| e.path == "meta/salt"),
                "meta/salt must be present in {reference}"
            );
            assert!(
                entries.iter().any(|e| e.path == "meta/machines.json"),
                "meta/machines.json must be present in {reference}"
            );
        }
    }
}

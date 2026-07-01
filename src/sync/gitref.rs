//! Git plumbing for the lore store refs under `refs/lore/*`.
//!
//! Every operation shells out to the user's `git` binary (no libgit2) so it
//! inherits their authentication, SSH config, credential helpers, and remotes
//! for free. `git` is already a hard dependency of the project. This mirrors the
//! style of [`crate::git`] but uses plumbing commands (`hash-object`, `mktree`
//! via a temporary index, `commit-tree`, `update-ref`, `cat-file`, `ls-tree`)
//! so the store ref lives entirely outside `refs/heads/*` and never checks out
//! into the working tree.
//!
//! All functions take an explicit repository path, run with that path as the
//! working directory, capture stderr, and return a [`SyncError::Git`] with the
//! command and stderr on failure.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use uuid::Uuid;

use super::SyncError;

/// The all-zeros object id git uses to mean "this ref does not yet exist".
///
/// Passed as the expected old value to a checked ref update to assert the ref
/// must be created (must not already exist).
pub const ZERO_OID: &str = "0000000000000000000000000000000000000000";

/// A single entry in a ref's tree.
///
/// Returned by [`read_tree`] so callers can rebuild a tree incrementally,
/// preserving the git object of any unchanged blob (content-addressed dedup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The git file mode (for example `100644` for a regular file).
    pub mode: String,
    /// The blob object SHA.
    pub sha: String,
    /// The full path of the entry within the tree (for example
    /// `sessions/<uuid>.enc`).
    pub path: String,
}

/// Writes raw bytes as a git blob and returns its object SHA.
///
/// Runs `git hash-object -w --stdin`, piping `data` to stdin. Because git
/// objects are content-addressed, writing identical bytes always yields the
/// same SHA, which is what makes incremental tree rebuilds dedup cleanly.
pub fn write_blob(repo: &Path, data: &[u8]) -> Result<String, SyncError> {
    let out = run_git_stdin(repo, &["hash-object", "-w", "--stdin"], data)?;
    Ok(stdout_to_string(out))
}

/// Reads the bytes of a blob by its object SHA.
///
/// Runs `git cat-file blob <sha>`.
pub fn read_blob(repo: &Path, sha: &str) -> Result<Vec<u8>, SyncError> {
    run_git(repo, &["cat-file", "blob", sha])
}

/// Reads the full (recursive) tree of a ref as a list of blob entries.
///
/// Runs `git ls-tree -r <reference>`. The reference may be a ref name, a commit
/// SHA, or a tree SHA. Returns an error if the reference cannot be resolved;
/// call [`ref_exists`] first when the ref may be absent.
pub fn read_tree(repo: &Path, reference: &str) -> Result<Vec<TreeEntry>, SyncError> {
    let out = run_git(repo, &["ls-tree", "-r", reference])?;
    let text = String::from_utf8_lossy(&out);

    let mut entries = Vec::new();
    for line in text.lines() {
        // Each line is "<mode> <type> <sha>\t<path>".
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split_whitespace();
        let mode = parts.next().unwrap_or("").to_string();
        let _object_type = parts.next();
        let sha = parts.next().unwrap_or("").to_string();
        if mode.is_empty() || sha.is_empty() {
            continue;
        }
        entries.push(TreeEntry {
            mode,
            sha,
            path: path.to_string(),
        });
    }

    Ok(entries)
}

/// Builds a new tree incrementally and returns its object SHA.
///
/// When `base` is `Some`, the new tree starts from that ref or tree-ish and only
/// the paths in `changes` are overwritten or added. Unchanged entries keep their
/// existing blob objects, giving content-addressed dedup so the repository grows
/// by near zero per sync. When `base` is `None`, the tree is built from scratch.
///
/// `changes` maps each path (for example `sessions/<uuid>.enc` or `meta/salt`)
/// to the blob SHA that should live at that path. Nested paths are handled
/// automatically.
///
/// Internally this uses a throwaway index file via `GIT_INDEX_FILE` so the
/// user's real index is never touched.
pub fn build_tree(
    repo: &Path,
    base: Option<&str>,
    changes: &BTreeMap<String, String>,
) -> Result<String, SyncError> {
    let git_dir = absolute_git_dir(repo)?;
    let index_path = git_dir.join(format!("lore-index-{}", Uuid::new_v4()));

    let result = build_tree_with_index(repo, &index_path, base, changes);

    // Best-effort cleanup of the temporary index regardless of outcome.
    let _ = std::fs::remove_file(&index_path);

    result
}

/// Performs the tree build against a specific temporary index path.
fn build_tree_with_index(
    repo: &Path,
    index_path: &Path,
    base: Option<&str>,
    changes: &BTreeMap<String, String>,
) -> Result<String, SyncError> {
    // Seed the temporary index from the base tree, if any.
    if let Some(base_ref) = base {
        run_git_index(repo, index_path, &["read-tree", base_ref])?;
    }

    // Overwrite or add only the changed entries.
    for (path, sha) in changes {
        let cacheinfo = format!("100644,{sha},{path}");
        run_git_index(
            repo,
            index_path,
            &["update-index", "--add", "--cacheinfo", &cacheinfo],
        )?;
    }

    let out = run_git_index(repo, index_path, &["write-tree"])?;
    Ok(stdout_to_string(out))
}

/// Creates a commit object for a tree and returns its SHA.
///
/// Runs `git commit-tree <tree> [-p <parent>] -m <message>`. The repository must
/// have a committer identity configured (the standard git requirement).
pub fn commit_tree(
    repo: &Path,
    tree_sha: &str,
    parent: Option<&str>,
    message: &str,
) -> Result<String, SyncError> {
    let mut args: Vec<&str> = vec!["commit-tree", tree_sha];
    if let Some(parent_sha) = parent {
        args.push("-p");
        args.push(parent_sha);
    }
    args.push("-m");
    args.push(message);

    let out = run_git(repo, &args)?;
    Ok(stdout_to_string(out))
}

/// Points a ref at a commit.
///
/// Runs `git update-ref <ref_name> <commit_sha>`. `ref_name` should be a full
/// ref name such as `refs/lore/sessions`.
pub fn update_ref(repo: &Path, ref_name: &str, commit_sha: &str) -> Result<(), SyncError> {
    run_git(repo, &["update-ref", ref_name, commit_sha])?;
    Ok(())
}

/// Points a ref at a commit only if it currently holds the expected value.
///
/// This is a compare-and-swap: it runs `git update-ref <ref> <new> <old>`, where
/// `old` is the value the ref must currently hold. When `old` is `None`, the
/// all-zeros OID ([`ZERO_OID`]) is used to assert the ref must not yet exist.
/// Git performs the comparison and the update atomically under a ref lock, so
/// two concurrent syncs in the same repo cannot silently clobber each other.
///
/// On a mismatch (the ref moved or already exists) this returns
/// [`SyncError::RefCasMismatch`] so a caller can re-read the ref and retry. Any
/// other failure is reported as [`SyncError::Git`].
pub fn update_ref_checked(
    repo: &Path,
    ref_name: &str,
    new_sha: &str,
    old: Option<&str>,
) -> Result<(), SyncError> {
    let old_value = old.unwrap_or(ZERO_OID);
    let args = ["update-ref", ref_name, new_sha, old_value];

    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let lowered = stderr.to_lowercase();
    // git reports a CAS failure via the ref lock: "cannot lock ref ...: is at X
    // but expected Y", or "reference already exists" when old is the zero OID.
    if lowered.contains("but expected")
        || lowered.contains("reference already exists")
        || lowered.contains("cannot lock ref")
        || lowered.contains("unable to resolve reference")
    {
        Err(SyncError::RefCasMismatch(format!(
            "{ref_name} did not hold expected value {old_value}: {}",
            stderr.trim()
        )))
    } else {
        Err(git_error(&args, &output.stderr))
    }
}

/// Resolves a ref to its commit SHA, or `None` if the ref does not exist.
pub fn resolve_ref(repo: &Path, ref_name: &str) -> Result<Option<String>, SyncError> {
    resolve_revision(repo, &format!("{ref_name}^{{commit}}"))
}

/// Resolves a ref to its tree SHA, or `None` if the ref does not exist.
pub fn resolve_tree(repo: &Path, ref_name: &str) -> Result<Option<String>, SyncError> {
    resolve_revision(repo, &format!("{ref_name}^{{tree}}"))
}

/// Returns whether a ref exists in the repository.
pub fn ref_exists(repo: &Path, ref_name: &str) -> Result<bool, SyncError> {
    Ok(resolve_ref(repo, ref_name)?.is_some())
}

/// Pushes a lore ref to a remote using an explicit refspec.
///
/// Runs `git push <remote> <ref_name>:<ref_name>`. The explicit refspec means
/// `refs/lore/*` syncs without the user configuring anything.
pub fn push(repo: &Path, remote: &str, ref_name: &str) -> Result<(), SyncError> {
    let refspec = format!("{ref_name}:{ref_name}");
    run_git(repo, &["push", remote, &refspec])?;
    Ok(())
}

/// Computes the remote-tracking ref name for a lore ref.
///
/// A lore ref such as `refs/lore/sessions` tracks a remote under
/// `refs/lore/remotes/<remote>/sessions`, mirroring how `refs/remotes/*` shadows
/// `refs/heads/*`. Fetching into this namespace (rather than into the live local
/// ref) lets the merge model read remote state even when the local ref has
/// diverged. Returns an error if `ref_name` is not under `refs/lore/`.
pub fn tracking_ref_name(remote: &str, ref_name: &str) -> Result<String, SyncError> {
    let name = ref_name.strip_prefix("refs/lore/").ok_or_else(|| {
        SyncError::Git(format!(
            "lore ref name must start with refs/lore/: {ref_name}"
        ))
    })?;
    Ok(format!("refs/lore/remotes/{remote}/{name}"))
}

/// Returns whether a remote advertises `ref_name`, without fetching.
///
/// Runs `git ls-remote --exit-code <remote> <ref_name>`. `--exit-code` makes git
/// exit 2 when no advertised ref matches, which is the expected "remote store not
/// initialized yet" state rather than a failure. Any other nonzero exit (a real
/// transport or auth problem) is surfaced as [`SyncError::Git`]. The exit code is
/// inspected directly so detection never depends on localized stderr text.
pub fn remote_ref_exists(repo: &Path, remote: &str, ref_name: &str) -> Result<bool, SyncError> {
    let args = ["ls-remote", "--exit-code", remote, ref_name];
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    if output.status.success() {
        Ok(true)
    } else if output.status.code() == Some(2) {
        // `--exit-code` reserves exit status 2 for "no matching ref advertised".
        Ok(false)
    } else {
        Err(git_error(&args, &output.stderr))
    }
}

/// Fetches a lore ref from a remote into its remote-tracking ref.
///
/// When the remote does not yet advertise `ref_name` (a remote store that has
/// never been pushed to), this returns `Ok(None)` so callers can treat it as an
/// expected empty state rather than a transport error. Remote ref existence is
/// probed with [`remote_ref_exists`] (via exit code, never stderr text), so a
/// genuine transport or auth failure still surfaces as [`SyncError::Git`].
///
/// Otherwise it runs
/// `git fetch <remote> +refs/lore/<name>:refs/lore/remotes/<remote>/<name>`. The
/// leading `+` forces the update, so the fetch succeeds even when the local
/// `refs/lore/<name>` (or a previous tracking ref) has diverged. The merge model
/// then reads the fetched state from the tracking ref, updates the local ref, and
/// pushes. Returns `Ok(Some(tracking))` with the tracking ref name that now holds
/// the remote state.
pub fn fetch(repo: &Path, remote: &str, ref_name: &str) -> Result<Option<String>, SyncError> {
    if !remote_ref_exists(repo, remote, ref_name)? {
        return Ok(None);
    }
    let tracking = tracking_ref_name(remote, ref_name)?;
    let refspec = format!("+{ref_name}:{tracking}");
    run_git(repo, &["fetch", remote, &refspec])?;
    Ok(Some(tracking))
}

/// Reads the tree of a lore ref's remote-tracking ref.
///
/// Convenience wrapper that resolves the tracking ref for `ref_name` on `remote`
/// (see [`tracking_ref_name`]) and returns its tree entries, or an empty vector
/// when nothing has been fetched into that tracking ref yet. Call after
/// [`fetch`] to read remote state for merging.
pub fn read_tracking_tree(
    repo: &Path,
    remote: &str,
    ref_name: &str,
) -> Result<Vec<TreeEntry>, SyncError> {
    let tracking = tracking_ref_name(remote, ref_name)?;
    if ref_exists(repo, &tracking)? {
        read_tree(repo, &tracking)
    } else {
        Ok(Vec::new())
    }
}

/// Adds a tracking-namespace lore fetch refspec to a remote's config (opt-in).
///
/// Configures `+refs/lore/*:refs/lore/remotes/<remote>/*` so a plain `git pull`
/// safely pre-populates the remote-tracking refs without force-moving the live
/// local `refs/lore/*`. The real merge into the local ref still happens through
/// `lore sync`; fetching into the live ref directly would bypass the merge model
/// and could make an unpushed local lore commit unreachable.
///
/// The operation is idempotent: if the tracking-namespace refspec is already
/// configured, nothing changes. An older `+refs/lore/*:refs/lore/*` entry written
/// by a previous build is migrated in place (removed, not duplicated) so plain
/// fetches stop clobbering the local ref.
pub fn add_lore_fetch_refspec(repo: &Path, remote: &str) -> Result<(), SyncError> {
    let key = format!("remote.{remote}.fetch");
    let desired = format!("+refs/lore/*:refs/lore/remotes/{remote}/*");
    let old_form = "+refs/lore/*:refs/lore/*";

    // `git config --get-all` exits non-zero when the key has no values, which is
    // not an error for our purposes, so run it directly and tolerate that.
    let output = Command::new("git")
        .current_dir(repo)
        .args(["config", "--get-all", &key])
        .output()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    let existing: Vec<String> = if output.status.success() {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().to_string())
            .collect()
    } else {
        Vec::new()
    };

    let has_desired = existing.iter().any(|line| line == &desired);
    let has_old_form = existing.iter().any(|line| line == old_form);

    // Remove any stale old-form entry so a plain fetch stops force-updating the
    // live local ref. The value pattern is an anchored regex matching the literal
    // old form (`+` and `*` escaped); other unrelated refspecs are left intact.
    if has_old_form {
        run_git(
            repo,
            &[
                "config",
                "--unset-all",
                &key,
                r"^\+refs/lore/\*:refs/lore/\*$",
            ],
        )?;
    }

    if !has_desired {
        run_git(repo, &["config", "--add", &key, &desired])?;
    }

    Ok(())
}

// ==================== Internal helpers ====================

/// Resolves a revision specifier to an object SHA, returning `None` when the
/// revision does not exist.
fn resolve_revision(repo: &Path, spec: &str) -> Result<Option<String>, SyncError> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", "--quiet", spec])
        .output()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    if output.status.success() {
        Ok(Some(stdout_to_string(output.stdout)))
    } else if output.stderr.is_empty() {
        // `--quiet` suppresses the "unknown revision" message and exits 1 when
        // the revision simply does not exist.
        Ok(None)
    } else {
        // A non-empty stderr indicates a real failure (for example, the path is
        // not a git repository).
        Err(git_error(&["rev-parse", "--verify", spec], &output.stderr))
    }
}

/// Returns the absolute path to the repository's git directory.
fn absolute_git_dir(repo: &Path) -> Result<PathBuf, SyncError> {
    let out = run_git(repo, &["rev-parse", "--absolute-git-dir"])?;
    Ok(PathBuf::from(stdout_to_string(out)))
}

/// Runs a git command in `repo`, returning stdout bytes on success.
fn run_git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, SyncError> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_error(args, &output.stderr))
    }
}

/// Runs a git command in `repo` with a dedicated `GIT_INDEX_FILE`.
fn run_git_index(repo: &Path, index_path: &Path, args: &[&str]) -> Result<Vec<u8>, SyncError> {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_INDEX_FILE", index_path)
        .args(args)
        .output()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_error(args, &output.stderr))
    }
}

/// Runs a git command in `repo`, piping `input` to its stdin.
fn run_git_stdin(repo: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>, SyncError> {
    let mut child = Command::new("git")
        .current_dir(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SyncError::Git(format!("failed to spawn git: {e}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| SyncError::Git("failed to open git stdin".to_string()))?;
        stdin.write_all(input)?;
        // stdin is dropped here, closing the pipe so git can finish.
    }

    let output = child.wait_with_output()?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_error(args, &output.stderr))
    }
}

/// Builds a [`SyncError::Git`] from a command and its stderr.
fn git_error(args: &[&str], stderr: &[u8]) -> SyncError {
    let message = String::from_utf8_lossy(stderr);
    SyncError::Git(format!("git {} failed: {}", args.join(" "), message.trim()))
}

/// Trims trailing whitespace from git stdout and returns it as a String.
fn stdout_to_string(out: Vec<u8>) -> String {
    String::from_utf8_lossy(&out).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a git command in tests, asserting success.
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

    /// Initializes a temp repo with a committer identity configured.
    ///
    /// Commit and tag signing are disabled in the repo's local config so the
    /// suite is deterministic regardless of the machine's global git config: a
    /// global `commit.gpgsign=true` with a passphrase-protected signing key would
    /// otherwise make committing prompt or hang during `cargo test sync::`.
    fn init_repo(repo: &Path) {
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.name", "Lore Test"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        git(repo, &["config", "commit.gpgsign", "false"]);
        git(repo, &["config", "tag.gpgsign", "false"]);
    }

    #[test]
    fn test_blob_tree_commit_ref_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        let session_id = "11111111-1111-1111-1111-111111111111";
        let enc_bytes = b"encrypted-session-bytes";
        let salt_bytes = b"salt-bytes-not-secret";

        let enc_sha = write_blob(repo, enc_bytes).unwrap();
        let salt_sha = write_blob(repo, salt_bytes).unwrap();

        let mut changes = BTreeMap::new();
        changes.insert(format!("sessions/{session_id}.enc"), enc_sha.clone());
        changes.insert("meta/salt".to_string(), salt_sha.clone());

        let tree = build_tree(repo, None, &changes).unwrap();
        let commit = commit_tree(repo, &tree, None, "lore: initial").unwrap();
        update_ref(repo, "refs/lore/sessions", &commit).unwrap();

        assert!(ref_exists(repo, "refs/lore/sessions").unwrap());
        assert_eq!(
            resolve_ref(repo, "refs/lore/sessions").unwrap(),
            Some(commit.clone())
        );
        assert!(resolve_tree(repo, "refs/lore/sessions").unwrap().is_some());

        let entries = read_tree(repo, "refs/lore/sessions").unwrap();
        assert_eq!(entries.len(), 2);

        let enc_entry = entries
            .iter()
            .find(|e| e.path == format!("sessions/{session_id}.enc"))
            .expect("session blob present");
        assert_eq!(enc_entry.sha, enc_sha);
        assert_eq!(read_blob(repo, &enc_entry.sha).unwrap(), enc_bytes);

        let salt_entry = entries
            .iter()
            .find(|e| e.path == "meta/salt")
            .expect("salt blob present");
        assert_eq!(read_blob(repo, &salt_entry.sha).unwrap(), salt_bytes);
    }

    #[test]
    fn test_incremental_rebuild_preserves_unchanged_blob() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        let first_id = "aaaaaaaa-0000-0000-0000-000000000001";
        let first_sha = write_blob(repo, b"first-session").unwrap();

        let mut changes = BTreeMap::new();
        changes.insert(format!("sessions/{first_id}.enc"), first_sha.clone());

        let tree1 = build_tree(repo, None, &changes).unwrap();
        let commit1 = commit_tree(repo, &tree1, None, "lore: first").unwrap();
        update_ref(repo, "refs/lore/sessions", &commit1).unwrap();

        // Add a second session, rebuilding from the existing tree.
        let second_id = "bbbbbbbb-0000-0000-0000-000000000002";
        let second_sha = write_blob(repo, b"second-session").unwrap();

        let mut changes2 = BTreeMap::new();
        changes2.insert(format!("sessions/{second_id}.enc"), second_sha.clone());

        let tree2 = build_tree(repo, Some("refs/lore/sessions"), &changes2).unwrap();
        let commit2 = commit_tree(repo, &tree2, Some(&commit1), "lore: second").unwrap();
        update_ref(repo, "refs/lore/sessions", &commit2).unwrap();

        let entries = read_tree(repo, "refs/lore/sessions").unwrap();
        assert_eq!(entries.len(), 2);

        // The first session's blob object is unchanged (content-addressed dedup).
        let first_entry = entries
            .iter()
            .find(|e| e.path == format!("sessions/{first_id}.enc"))
            .expect("first session still present");
        assert_eq!(first_entry.sha, first_sha);

        // The second session is present too.
        let second_entry = entries
            .iter()
            .find(|e| e.path == format!("sessions/{second_id}.enc"))
            .expect("second session present");
        assert_eq!(second_entry.sha, second_sha);
    }

    #[test]
    fn test_push_and_fetch_between_repos() {
        let remote_dir = tempfile::tempdir().unwrap();
        let remote = remote_dir.path();
        git(remote, &["init", "--bare", "-q"]);
        let remote_url = remote.to_str().unwrap();

        // Destination repo has its OWN divergent local lore ref before fetching.
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = dst_dir.path();
        init_repo(dst);
        git(dst, &["remote", "add", "origin", remote_url]);

        let local_blob = write_blob(dst, b"local-only-reasoning").unwrap();
        let mut local_changes = BTreeMap::new();
        local_changes.insert("sessions/local.enc".to_string(), local_blob);
        let local_tree = build_tree(dst, None, &local_changes).unwrap();
        let local_commit = commit_tree(dst, &local_tree, None, "lore: local").unwrap();
        update_ref(dst, "refs/lore/sessions", &local_commit).unwrap();

        // First fetch against a remote with no lore ref is an expected empty
        // state, not an error: nothing was fetched.
        assert_eq!(fetch(dst, "origin", "refs/lore/sessions").unwrap(), None);

        // Source repo creates and pushes a lore ref.
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path();
        init_repo(src);
        git(src, &["remote", "add", "origin", remote_url]);

        let blob = write_blob(src, b"reasoning-history").unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("sessions/x.enc".to_string(), blob.clone());
        let tree = build_tree(src, None, &changes).unwrap();
        let commit = commit_tree(src, &tree, None, "lore: push").unwrap();
        update_ref(src, "refs/lore/sessions", &commit).unwrap();
        push(src, "origin", "refs/lore/sessions").unwrap();

        // Now that the remote advertises the ref, fetch must succeed despite the
        // divergent local ref and report the tracking ref it wrote into.
        let tracking = fetch(dst, "origin", "refs/lore/sessions").unwrap();
        assert_eq!(
            tracking.as_deref(),
            Some("refs/lore/remotes/origin/sessions")
        );

        // The local ref is untouched by the fetch.
        assert_eq!(
            resolve_ref(dst, "refs/lore/sessions").unwrap(),
            Some(local_commit)
        );

        // Remote contents are readable from the tracking ref.
        let entries = read_tracking_tree(dst, "origin", "refs/lore/sessions").unwrap();
        let entry = entries
            .iter()
            .find(|e| e.path == "sessions/x.enc")
            .expect("session transferred into tracking ref");
        assert_eq!(read_blob(dst, &entry.sha).unwrap(), b"reasoning-history");

        // Fetching again (now that the tracking ref also exists) still succeeds.
        let tracking2 = fetch(dst, "origin", "refs/lore/sessions").unwrap();
        assert_eq!(
            tracking2.as_deref(),
            Some("refs/lore/remotes/origin/sessions")
        );
    }

    #[test]
    fn test_tracking_ref_name_requires_lore_prefix() {
        assert_eq!(
            tracking_ref_name("origin", "refs/lore/sessions").unwrap(),
            "refs/lore/remotes/origin/sessions"
        );
        assert!(tracking_ref_name("origin", "refs/heads/main").is_err());
    }

    #[test]
    fn test_read_tracking_tree_empty_when_not_fetched() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        let entries = read_tracking_tree(repo, "origin", "refs/lore/sessions").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_update_ref_checked_create_and_cas() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        let blob = write_blob(repo, b"v1").unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("sessions/a.enc".to_string(), blob);
        let tree = build_tree(repo, None, &changes).unwrap();
        let commit1 = commit_tree(repo, &tree, None, "lore: v1").unwrap();
        let commit2 = commit_tree(repo, &tree, Some(&commit1), "lore: v2").unwrap();

        // Creating the ref (expected old = None => zero OID) succeeds.
        update_ref_checked(repo, "refs/lore/sessions", &commit1, None).unwrap();
        assert_eq!(
            resolve_ref(repo, "refs/lore/sessions").unwrap(),
            Some(commit1.clone())
        );

        // Creating again with expected-not-exist must fail as a CAS mismatch.
        let err = update_ref_checked(repo, "refs/lore/sessions", &commit2, None).unwrap_err();
        assert!(matches!(err, SyncError::RefCasMismatch(_)));

        // Updating with the correct expected old value succeeds.
        update_ref_checked(repo, "refs/lore/sessions", &commit2, Some(&commit1)).unwrap();
        assert_eq!(
            resolve_ref(repo, "refs/lore/sessions").unwrap(),
            Some(commit2.clone())
        );

        // Updating with a stale expected old value fails as a CAS mismatch and
        // leaves the ref unchanged.
        let err =
            update_ref_checked(repo, "refs/lore/sessions", &commit1, Some(&commit1)).unwrap_err();
        assert!(matches!(err, SyncError::RefCasMismatch(_)));
        assert_eq!(
            resolve_ref(repo, "refs/lore/sessions").unwrap(),
            Some(commit2)
        );
    }

    #[test]
    fn test_lore_ref_not_in_branches_or_working_tree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        // Make a normal commit on the default branch.
        std::fs::write(repo.join("README.md"), "hello").unwrap();
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-q", "-m", "init"]);

        // Build a lore ref with session and meta paths.
        let blob = write_blob(repo, b"reasoning").unwrap();
        let salt = write_blob(repo, b"salt").unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("sessions/y.enc".to_string(), blob);
        changes.insert("meta/salt".to_string(), salt);
        let tree = build_tree(repo, None, &changes).unwrap();
        let commit = commit_tree(repo, &tree, None, "lore: hidden").unwrap();
        update_ref(repo, "refs/lore/sessions", &commit).unwrap();

        // refs/lore/* must not appear among branches.
        let branches = run_git(repo, &["branch", "--format=%(refname)"]).unwrap();
        let branch_text = String::from_utf8_lossy(&branches);
        assert!(
            !branch_text.contains("refs/lore"),
            "lore ref leaked into branches: {branch_text}"
        );

        // The lore tree's paths must not be checked out into the working tree.
        assert!(!repo.join("sessions").exists());
        assert!(!repo.join("meta").exists());
    }

    #[test]
    fn test_resolve_ref_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);

        assert_eq!(resolve_ref(repo, "refs/lore/sessions").unwrap(), None);
        assert!(!ref_exists(repo, "refs/lore/sessions").unwrap());
    }

    #[test]
    fn test_add_lore_fetch_refspec_idempotent() {
        let remote_dir = tempfile::tempdir().unwrap();
        git(remote_dir.path(), &["init", "--bare", "-q"]);

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(
            repo,
            &[
                "remote",
                "add",
                "origin",
                remote_dir.path().to_str().unwrap(),
            ],
        );

        add_lore_fetch_refspec(repo, "origin").unwrap();
        // Second call must not add a duplicate.
        add_lore_fetch_refspec(repo, "origin").unwrap();

        let out = run_git(repo, &["config", "--get-all", "remote.origin.fetch"]).unwrap();
        let text = String::from_utf8_lossy(&out);
        let tracking = text
            .lines()
            .filter(|l| l.trim() == "+refs/lore/*:refs/lore/remotes/origin/*")
            .count();
        assert_eq!(
            tracking, 1,
            "tracking refspec should appear exactly once: {text}"
        );
        // The old live-ref form must never be written.
        assert!(
            !text.lines().any(|l| l.trim() == "+refs/lore/*:refs/lore/*"),
            "old-form refspec must not be configured: {text}"
        );
    }

    #[test]
    fn test_add_lore_fetch_refspec_migrates_old_form() {
        let remote_dir = tempfile::tempdir().unwrap();
        git(remote_dir.path(), &["init", "--bare", "-q"]);

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(
            repo,
            &[
                "remote",
                "add",
                "origin",
                remote_dir.path().to_str().unwrap(),
            ],
        );

        // Simulate a stale old-form refspec written by an earlier build.
        git(
            repo,
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "+refs/lore/*:refs/lore/*",
            ],
        );

        add_lore_fetch_refspec(repo, "origin").unwrap();

        let out = run_git(repo, &["config", "--get-all", "remote.origin.fetch"]).unwrap();
        let text = String::from_utf8_lossy(&out);

        // The old form is migrated away, not left alongside the new one.
        assert!(
            !text.lines().any(|l| l.trim() == "+refs/lore/*:refs/lore/*"),
            "old-form refspec should be removed: {text}"
        );
        let tracking = text
            .lines()
            .filter(|l| l.trim() == "+refs/lore/*:refs/lore/remotes/origin/*")
            .count();
        assert_eq!(
            tracking, 1,
            "tracking refspec should appear exactly once after migration: {text}"
        );

        // A subsequent call stays idempotent.
        add_lore_fetch_refspec(repo, "origin").unwrap();
        let out = run_git(repo, &["config", "--get-all", "remote.origin.fetch"]).unwrap();
        let text = String::from_utf8_lossy(&out);
        let tracking = text
            .lines()
            .filter(|l| l.trim() == "+refs/lore/*:refs/lore/remotes/origin/*")
            .count();
        assert_eq!(tracking, 1, "migration must stay idempotent: {text}");
    }

    #[test]
    fn test_remote_ref_exists_reflects_remote_state() {
        let remote_dir = tempfile::tempdir().unwrap();
        let remote = remote_dir.path();
        git(remote, &["init", "--bare", "-q"]);
        let remote_url = remote.to_str().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        init_repo(repo);
        git(repo, &["remote", "add", "origin", remote_url]);

        // No lore ref on the remote yet.
        assert!(!remote_ref_exists(repo, "origin", "refs/lore/sessions").unwrap());

        // Push one, then it is advertised.
        let blob = write_blob(repo, b"reasoning").unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("sessions/x.enc".to_string(), blob);
        let tree = build_tree(repo, None, &changes).unwrap();
        let commit = commit_tree(repo, &tree, None, "lore: push").unwrap();
        update_ref(repo, "refs/lore/sessions", &commit).unwrap();
        push(repo, "origin", "refs/lore/sessions").unwrap();

        assert!(remote_ref_exists(repo, "origin", "refs/lore/sessions").unwrap());
    }

    #[test]
    fn test_run_git_error_on_non_repo() {
        let dir = tempfile::tempdir().unwrap();
        // No git init here, so this is not a repository.
        let result = read_tree(dir.path(), "refs/lore/sessions");
        assert!(result.is_err());
    }
}

//! Git hooks management command.
//!
//! Provides functionality to install, uninstall, and check the status of
//! git hooks that integrate Lore with the git workflow. The hooks enable
//! automatic session linking after commits, session references in commit
//! messages, and best-effort reasoning-history sync when you push.
//!
//! The pre-push hook is the recommended way to automate `lore sync`: reasoning
//! rides on the git pushes you already do, so the background daemon is optional
//! for sync.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Marker comment to identify Lore-managed hooks.
const LORE_HOOK_MARKER: &str = "# Lore hook - managed by lore hooks install";

/// Post-commit hook script content.
///
/// This hook runs after each commit and links any currently active AI
/// development sessions to the commit. This enables forward auto-linking
/// where sessions are linked as commits happen, rather than retroactively.
const POST_COMMIT_HOOK: &str = r#"#!/bin/sh
# Lore post-commit hook
# Lore hook - managed by lore hooks install

# Link any active sessions to this commit
if command -v lore >/dev/null 2>&1; then
    lore link --current --commit HEAD 2>/dev/null || true
fi
"#;

/// Prepare-commit-msg hook script content.
const PREPARE_COMMIT_MSG_HOOK: &str = r#"#!/bin/sh
# Lore prepare-commit-msg hook - add session references
# Lore hook - managed by lore hooks install

COMMIT_MSG_FILE=$1
COMMIT_SOURCE=$2

# Only run for regular commits (not merge, squash, etc.)
if [ "$COMMIT_SOURCE" = "" ] || [ "$COMMIT_SOURCE" = "message" ]; then
    if command -v lore >/dev/null 2>&1; then
        # Get active sessions that might be related to this commit
        # This is a placeholder - full implementation would query lore
        :
    fi
fi
"#;

/// Pre-push hook script content.
///
/// This hook runs before each `git push` and best-effort syncs the repo's
/// encrypted reasoning history (`refs/lore/sessions`) alongside the code you are
/// pushing. It never blocks the push: a sync failure prints a brief warning and
/// the hook still exits 0.
///
/// Re-entry guard: `lore sync` internally runs `git push` to publish the lore
/// refs, which would fire this pre-push hook again and loop forever. The hook
/// exports `LORE_SYNC_HOOK=1` before invoking `lore sync`, and exits early if
/// that variable is already set. Because it is exported, the guard is inherited
/// by `lore sync` and by the nested `git push` it spawns, so that inner push's
/// pre-push hook sees the guard and skips.
const PRE_PUSH_HOOK: &str = r#"#!/bin/sh
# Lore pre-push hook - sync reasoning history when you push code
# Lore hook - managed by lore hooks install

# Re-entry guard: lore sync runs `git push` internally to publish refs/lore/*,
# which fires this same pre-push hook again. If the guard is already set we are
# inside that nested push, so skip to avoid an infinite loop. It is exported so
# the guard is inherited by lore sync and by the git push it spawns.
if [ -n "$LORE_SYNC_HOOK" ]; then
    exit 0
fi
export LORE_SYNC_HOOK=1

# git passes the pushed remote's name as $1 (and its URL as $2).
remote="$1"

# Best-effort: a sync failure (network, merge, or unconfigured store) must never
# block the push. The --quiet mode no-ops when this repo's lore store is not set
# up and never prompts for a passphrase.
if command -v lore >/dev/null 2>&1; then
    if ! lore sync --remote "$remote" --quiet; then
        echo "lore: sync failed; your git push was not affected" >&2
    fi
fi

exit 0
"#;

/// Hook types that Lore manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    /// Runs after each commit to auto-link sessions.
    PostCommit,
    /// Runs before commit message editor to add session references.
    PrepareCommitMsg,
    /// Runs before each push to best-effort sync reasoning history.
    PrePush,
}

impl HookType {
    /// Returns the filename for this hook type.
    fn filename(&self) -> &'static str {
        match self {
            HookType::PostCommit => "post-commit",
            HookType::PrepareCommitMsg => "prepare-commit-msg",
            HookType::PrePush => "pre-push",
        }
    }

    /// Returns the script content for this hook type.
    fn content(&self) -> &'static str {
        match self {
            HookType::PostCommit => POST_COMMIT_HOOK,
            HookType::PrepareCommitMsg => PREPARE_COMMIT_MSG_HOOK,
            HookType::PrePush => PRE_PUSH_HOOK,
        }
    }

    /// Returns all managed hook types.
    fn all() -> &'static [HookType] {
        &[
            HookType::PostCommit,
            HookType::PrepareCommitMsg,
            HookType::PrePush,
        ]
    }
}

/// Subcommands for the hooks command.
#[derive(Subcommand)]
pub enum HooksCommand {
    /// Install git hooks in the current repository
    #[command(long_about = "Installs Lore's git hooks in the current repository's\n\
        .git/hooks directory. The post-commit hook automatically\n\
        links sessions to commits using time and file overlap. The\n\
        pre-push hook best-effort syncs reasoning history when you push\n\
        (no daemon required); it never blocks the push.\n\
        Existing hooks are backed up before being replaced.")]
    Install {
        /// Overwrite existing hooks (backs up originals)
        #[arg(long)]
        #[arg(long_help = "Replace existing hooks that are not managed by Lore.\n\
            The original hooks are saved as <hook>.backup and can\n\
            be restored with 'lore hooks uninstall'.")]
        force: bool,
    },

    /// Uninstall git hooks from the current repository
    #[command(long_about = "Removes Lore's git hooks from the current repository.\n\
        Only removes hooks that Lore installed (identified by marker).\n\
        Restores backed-up hooks if they exist.")]
    Uninstall,

    /// Show status of installed hooks
    #[command(long_about = "Shows which git hooks are currently installed and\n\
        whether they are managed by Lore or are third-party hooks.")]
    Status,
}

/// Arguments for the hooks command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore hooks install         Install hooks (skips existing)\n    \
    lore hooks install --force Replace existing hooks\n    \
    lore hooks uninstall       Remove Lore hooks\n    \
    lore hooks status          Check installed hooks")]
pub struct Args {
    /// Hooks subcommand to run
    #[command(subcommand)]
    pub command: HooksCommand,
}

/// Executes the hooks command.
///
/// Dispatches to the appropriate subcommand handler.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        HooksCommand::Install { force } => run_install(force),
        HooksCommand::Uninstall => run_uninstall(),
        HooksCommand::Status => run_status(),
    }
}

/// Installs Lore git hooks in the current repository.
///
/// Creates hook scripts in `.git/hooks/` that integrate with Lore.
/// Existing hooks are backed up before being replaced when using --force.
fn run_install(force: bool) -> Result<()> {
    let hooks_dir = get_hooks_dir()?;
    println!("Installing Lore hooks in {}", hooks_dir.display());
    println!();

    let mut installed_count = 0;
    let mut skipped_count = 0;

    for hook_type in HookType::all() {
        let hook_path = hooks_dir.join(hook_type.filename());
        let status = install_hook(&hook_path, *hook_type, force)?;

        match status {
            InstallStatus::Installed => {
                println!("  {} {}", "Installed".green(), hook_type.filename());
                installed_count += 1;
            }
            InstallStatus::Replaced => {
                println!(
                    "  {} {} (backed up existing to {}.backup)",
                    "Replaced".yellow(),
                    hook_type.filename(),
                    hook_type.filename()
                );
                installed_count += 1;
            }
            InstallStatus::Skipped => {
                println!(
                    "  {} {} (use --force to overwrite)",
                    "Skipped".yellow(),
                    hook_type.filename()
                );
                skipped_count += 1;
            }
            InstallStatus::AlreadyInstalled => {
                println!(
                    "  {} {} (already a Lore hook)",
                    "Skipped".dimmed(),
                    hook_type.filename()
                );
                skipped_count += 1;
            }
        }
    }

    println!();
    if installed_count > 0 {
        println!(
            "Successfully installed {} hook(s).",
            installed_count.to_string().green()
        );
    }
    if skipped_count > 0 && !force {
        println!("{}", "Use --force to overwrite existing hooks.".dimmed());
    }

    Ok(())
}

/// Status of a hook installation attempt.
enum InstallStatus {
    /// Hook was freshly installed.
    Installed,
    /// Existing hook was backed up and replaced.
    Replaced,
    /// Hook already exists and was not replaced.
    Skipped,
    /// Hook is already a Lore-managed hook.
    AlreadyInstalled,
}

/// Installs a single hook.
///
/// Returns the status of the installation attempt.
fn install_hook(hook_path: &Path, hook_type: HookType, force: bool) -> Result<InstallStatus> {
    if hook_path.exists() {
        // Check if it's already a Lore hook
        let existing_content = fs::read_to_string(hook_path)
            .with_context(|| format!("Failed to read existing hook: {}", hook_path.display()))?;

        if existing_content.contains(LORE_HOOK_MARKER) {
            // Already a Lore hook, update it
            write_hook(hook_path, hook_type)?;
            return Ok(InstallStatus::AlreadyInstalled);
        }

        if !force {
            return Ok(InstallStatus::Skipped);
        }

        // Backup existing hook
        let backup_path = hook_path.with_extension("backup");
        fs::rename(hook_path, &backup_path)
            .with_context(|| format!("Failed to backup hook to {}", backup_path.display()))?;

        write_hook(hook_path, hook_type)?;
        Ok(InstallStatus::Replaced)
    } else {
        write_hook(hook_path, hook_type)?;
        Ok(InstallStatus::Installed)
    }
}

/// Writes a hook script to the specified path.
///
/// Sets the executable bit on Unix systems.
fn write_hook(hook_path: &Path, hook_type: HookType) -> Result<()> {
    fs::write(hook_path, hook_type.content())
        .with_context(|| format!("Failed to write hook: {}", hook_path.display()))?;

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(hook_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(hook_path, perms)
            .with_context(|| format!("Failed to set permissions on {}", hook_path.display()))?;
    }

    Ok(())
}

/// Uninstalls Lore git hooks from the current repository.
///
/// Only removes hooks that were installed by Lore (identified by marker comment).
/// Restores backup hooks if they exist.
fn run_uninstall() -> Result<()> {
    let hooks_dir = get_hooks_dir()?;
    println!("Uninstalling Lore hooks from {}", hooks_dir.display());
    println!();

    let mut removed_count = 0;
    let mut restored_count = 0;
    let mut not_found_count = 0;

    for hook_type in HookType::all() {
        let hook_path = hooks_dir.join(hook_type.filename());

        if !hook_path.exists() {
            println!(
                "  {} {} (not installed)",
                "Skipped".dimmed(),
                hook_type.filename()
            );
            not_found_count += 1;
            continue;
        }

        // Check if it's a Lore hook
        let content = fs::read_to_string(&hook_path)
            .with_context(|| format!("Failed to read hook: {}", hook_path.display()))?;

        if !content.contains(LORE_HOOK_MARKER) {
            println!(
                "  {} {} (not a Lore hook)",
                "Skipped".yellow(),
                hook_type.filename()
            );
            continue;
        }

        // Remove the hook
        fs::remove_file(&hook_path)
            .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
        removed_count += 1;

        // Check for backup to restore
        let backup_path = hook_path.with_extension("backup");
        if backup_path.exists() {
            fs::rename(&backup_path, &hook_path)
                .with_context(|| format!("Failed to restore backup: {}", backup_path.display()))?;
            println!(
                "  {} {} (restored from backup)",
                "Removed".green(),
                hook_type.filename()
            );
            restored_count += 1;
        } else {
            println!("  {} {}", "Removed".green(), hook_type.filename());
        }
    }

    println!();
    if removed_count > 0 {
        println!("Removed {} hook(s).", removed_count.to_string().green());
        if restored_count > 0 {
            println!(
                "Restored {} original hook(s) from backup.",
                restored_count.to_string().green()
            );
        }
    } else if not_found_count == HookType::all().len() {
        println!("{}", "No Lore hooks were installed.".yellow());
    }

    Ok(())
}

/// Shows the status of Lore git hooks.
///
/// Reports which hooks are installed and whether they are Lore-managed.
fn run_status() -> Result<()> {
    let hooks_dir = get_hooks_dir()?;

    println!("Git hooks status:");
    println!();

    for hook_type in HookType::all() {
        let hook_path = hooks_dir.join(hook_type.filename());
        let status = get_hook_status(&hook_path)?;

        let status_str = match status {
            HookStatus::Lore => "installed".green().to_string(),
            HookStatus::Other => "other hook installed".yellow().to_string(),
            HookStatus::None => "not installed".dimmed().to_string(),
        };

        println!(
            "  {:<20} {}",
            format!("{}:", hook_type.filename()),
            status_str
        );
    }

    Ok(())
}

/// Status of a hook file.
enum HookStatus {
    /// Lore hook is installed.
    Lore,
    /// Another (non-Lore) hook is installed.
    Other,
    /// No hook is installed.
    None,
}

/// Gets the status of a hook file.
fn get_hook_status(hook_path: &Path) -> Result<HookStatus> {
    if !hook_path.exists() {
        return Ok(HookStatus::None);
    }

    let content = fs::read_to_string(hook_path)
        .with_context(|| format!("Failed to read hook: {}", hook_path.display()))?;

    if content.contains(LORE_HOOK_MARKER) {
        Ok(HookStatus::Lore)
    } else {
        Ok(HookStatus::Other)
    }
}

/// Gets the path to the git hooks directory.
///
/// Discovers the git repository and returns the path to `.git/hooks/`.
fn get_hooks_dir() -> Result<PathBuf> {
    let repo = git2::Repository::discover(".")
        .context("Not in a git repository. Run this command from within a git repository.")?;

    let git_dir = repo.path();
    let hooks_dir = git_dir.join("hooks");

    // Create hooks directory if it doesn't exist
    if !hooks_dir.exists() {
        fs::create_dir_all(&hooks_dir).with_context(|| {
            format!("Failed to create hooks directory: {}", hooks_dir.display())
        })?;
    }

    Ok(hooks_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Creates a temporary git repository for testing.
    fn create_test_repo() -> Result<(TempDir, PathBuf)> {
        let temp_dir = TempDir::new()?;
        let repo_path = temp_dir.path();

        // Initialize git repository
        git2::Repository::init(repo_path)?;

        let hooks_dir = repo_path.join(".git").join("hooks");
        fs::create_dir_all(&hooks_dir)?;

        Ok((temp_dir, hooks_dir))
    }

    #[test]
    fn test_hook_type_filename() {
        assert_eq!(HookType::PostCommit.filename(), "post-commit");
        assert_eq!(HookType::PrepareCommitMsg.filename(), "prepare-commit-msg");
    }

    #[test]
    fn test_hook_type_content_contains_marker() {
        for hook_type in HookType::all() {
            assert!(
                hook_type.content().contains(LORE_HOOK_MARKER),
                "Hook {} should contain the Lore marker",
                hook_type.filename()
            );
        }
    }

    #[test]
    fn test_hook_type_content_is_valid_shell_script() {
        for hook_type in HookType::all() {
            assert!(
                hook_type.content().starts_with("#!/bin/sh"),
                "Hook {} should start with shebang",
                hook_type.filename()
            );
        }
    }

    #[test]
    fn test_post_commit_hook_calls_link_current() {
        let content = HookType::PostCommit.content();
        assert!(
            content.contains("lore link --current --commit HEAD"),
            "Post-commit hook should call 'lore link --current --commit HEAD'"
        );
    }

    #[test]
    fn test_install_hook_fresh() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        let status = install_hook(&hook_path, HookType::PostCommit, false)?;

        assert!(matches!(status, InstallStatus::Installed));
        assert!(hook_path.exists());

        let content = fs::read_to_string(&hook_path)?;
        assert!(content.contains(LORE_HOOK_MARKER));

        Ok(())
    }

    #[test]
    fn test_install_hook_skips_existing() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        // Create existing hook
        fs::write(&hook_path, "#!/bin/sh\necho 'existing hook'")?;

        let status = install_hook(&hook_path, HookType::PostCommit, false)?;

        assert!(matches!(status, InstallStatus::Skipped));

        // Original content should be preserved
        let content = fs::read_to_string(&hook_path)?;
        assert!(content.contains("existing hook"));

        Ok(())
    }

    #[test]
    fn test_install_hook_force_creates_backup() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");
        let backup_path = hooks_dir.join("post-commit.backup");

        // Create existing hook
        fs::write(&hook_path, "#!/bin/sh\necho 'existing hook'")?;

        let status = install_hook(&hook_path, HookType::PostCommit, true)?;

        assert!(matches!(status, InstallStatus::Replaced));
        assert!(backup_path.exists());

        // Backup should contain original content
        let backup_content = fs::read_to_string(&backup_path)?;
        assert!(backup_content.contains("existing hook"));

        // New hook should be Lore hook
        let new_content = fs::read_to_string(&hook_path)?;
        assert!(new_content.contains(LORE_HOOK_MARKER));

        Ok(())
    }

    #[test]
    fn test_install_hook_updates_existing_lore_hook() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        // Create existing Lore hook
        let old_content = format!("#!/bin/sh\n{LORE_HOOK_MARKER}\nold version");
        fs::write(&hook_path, &old_content)?;

        let status = install_hook(&hook_path, HookType::PostCommit, false)?;

        assert!(matches!(status, InstallStatus::AlreadyInstalled));

        // Should be updated to current content
        let content = fs::read_to_string(&hook_path)?;
        assert_eq!(content, POST_COMMIT_HOOK);

        Ok(())
    }

    #[test]
    fn test_get_hook_status_not_installed() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        let status = get_hook_status(&hook_path)?;

        assert!(matches!(status, HookStatus::None));

        Ok(())
    }

    #[test]
    fn test_get_hook_status_lore_installed() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        fs::write(&hook_path, POST_COMMIT_HOOK)?;

        let status = get_hook_status(&hook_path)?;

        assert!(matches!(status, HookStatus::Lore));

        Ok(())
    }

    #[test]
    fn test_get_hook_status_other_installed() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        fs::write(&hook_path, "#!/bin/sh\necho 'other hook'")?;

        let status = get_hook_status(&hook_path)?;

        assert!(matches!(status, HookStatus::Other));

        Ok(())
    }

    #[test]
    fn test_pre_push_hook_included_in_all() {
        assert!(
            HookType::all().contains(&HookType::PrePush),
            "pre-push hook must be part of the managed set"
        );
        assert_eq!(HookType::PrePush.filename(), "pre-push");
    }

    #[test]
    fn test_pre_push_hook_content() {
        let content = HookType::PrePush.content();
        // Best-effort sync via the quiet mode, passing the pushed remote through.
        assert!(
            content.contains(r#"lore sync --remote "$remote" --quiet"#),
            "pre-push hook must call quiet sync with the pushed remote"
        );
        // git provides the remote name as the first argument.
        assert!(
            content.contains(r#"remote="$1""#),
            "pre-push hook must read the remote name from $1"
        );
        // Re-entry guard export and early-exit check.
        assert!(
            content.contains("export LORE_SYNC_HOOK=1"),
            "pre-push hook must export the re-entry guard"
        );
        assert!(
            content.contains(r#"if [ -n "$LORE_SYNC_HOOK" ]; then"#),
            "pre-push hook must skip when the guard is already set"
        );
        // Best-effort: it always exits 0 so it never blocks the push.
        assert!(
            content.trim_end().ends_with("exit 0"),
            "pre-push hook must exit 0 so it never blocks the push"
        );
    }

    #[test]
    fn test_pre_push_install_uninstall_round_trip() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("pre-push");

        let status = install_hook(&hook_path, HookType::PrePush, false)?;
        assert!(matches!(status, InstallStatus::Installed));
        assert!(hook_path.exists());
        assert_eq!(fs::read_to_string(&hook_path)?, PRE_PUSH_HOOK);

        // Uninstall removes only the Lore-managed hook file.
        assert!(matches!(get_hook_status(&hook_path)?, HookStatus::Lore));
        fs::remove_file(&hook_path)?;
        assert!(matches!(get_hook_status(&hook_path)?, HookStatus::None));

        Ok(())
    }

    #[test]
    fn test_pre_push_install_is_idempotent() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("pre-push");

        install_hook(&hook_path, HookType::PrePush, false)?;
        let first = fs::read_to_string(&hook_path)?;

        // A second install recognizes the existing Lore hook and rewrites the
        // canonical content rather than skipping or duplicating it.
        let status = install_hook(&hook_path, HookType::PrePush, false)?;
        assert!(matches!(status, InstallStatus::AlreadyInstalled));
        let second = fs::read_to_string(&hook_path)?;
        assert_eq!(first, second, "reinstall must be idempotent");
        assert_eq!(second, PRE_PUSH_HOOK);

        Ok(())
    }

    /// Runs the pre-push hook script under `sh` with a fake `lore` on `PATH`.
    ///
    /// The fake `lore` touches `marker` when invoked. Returns whether the marker
    /// was created (whether the hook actually called `lore sync`). `guard_set`
    /// controls whether `LORE_SYNC_HOOK` is already exported when the hook runs.
    #[cfg(unix)]
    fn run_pre_push_hook(guard_set: bool) -> bool {
        use std::process::Command;

        let dir = TempDir::new().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();

        let marker = dir.path().join("lore-was-called");
        let fake_lore = bin.join("lore");
        fs::write(
            &fake_lore,
            format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
        )
        .unwrap();
        let mut perms = fs::metadata(&fake_lore).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_lore, perms).unwrap();

        let hook_path = dir.path().join("pre-push");
        write_hook(&hook_path, HookType::PrePush).unwrap();

        // PATH contains the fake lore first, plus system dirs for `sh`/`touch`.
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let mut cmd = Command::new("sh");
        cmd.arg(&hook_path)
            .arg("origin")
            .arg("https://example.com/repo.git")
            .env("PATH", path);
        if guard_set {
            cmd.env("LORE_SYNC_HOOK", "1");
        } else {
            cmd.env_remove("LORE_SYNC_HOOK");
        }

        let status = cmd.status().unwrap();
        assert!(status.success(), "pre-push hook must always exit 0");

        marker.exists()
    }

    #[cfg(unix)]
    #[test]
    fn test_pre_push_hook_runs_sync_when_guard_unset() {
        assert!(
            run_pre_push_hook(false),
            "pre-push hook must invoke lore sync when the guard is not set"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_pre_push_hook_skips_when_guard_set() {
        assert!(
            !run_pre_push_hook(true),
            "pre-push hook must skip when the re-entry guard is already set"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_write_hook_sets_executable() -> Result<()> {
        let (_temp_dir, hooks_dir) = create_test_repo()?;
        let hook_path = hooks_dir.join("post-commit");

        write_hook(&hook_path, HookType::PostCommit)?;

        let metadata = fs::metadata(&hook_path)?;
        let mode = metadata.permissions().mode();

        // Check that owner execute bit is set
        assert!(mode & 0o100 != 0, "Hook should be executable");

        Ok(())
    }
}

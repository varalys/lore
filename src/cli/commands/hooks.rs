//! Git hooks management command.
//!
//! Provides functionality to install, uninstall, and check the status of
//! git hooks that integrate Lore with the git workflow. The hooks enable
//! automatic session linking after commits and session references in commit
//! messages.

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
const POST_COMMIT_HOOK: &str = r#"#!/bin/sh
# Lore post-commit hook - auto-link sessions to commits
# Lore hook - managed by lore hooks install

# Only run if lore is available
if command -v lore >/dev/null 2>&1; then
    lore link --auto --commit HEAD 2>/dev/null || true
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

/// Hook types that Lore manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    /// Runs after each commit to auto-link sessions.
    PostCommit,
    /// Runs before commit message editor to add session references.
    PrepareCommitMsg,
}

impl HookType {
    /// Returns the filename for this hook type.
    fn filename(&self) -> &'static str {
        match self {
            HookType::PostCommit => "post-commit",
            HookType::PrepareCommitMsg => "prepare-commit-msg",
        }
    }

    /// Returns the script content for this hook type.
    fn content(&self) -> &'static str {
        match self {
            HookType::PostCommit => POST_COMMIT_HOOK,
            HookType::PrepareCommitMsg => PREPARE_COMMIT_MSG_HOOK,
        }
    }

    /// Returns all managed hook types.
    fn all() -> &'static [HookType] {
        &[HookType::PostCommit, HookType::PrepareCommitMsg]
    }
}

/// Subcommands for the hooks command.
#[derive(Subcommand)]
pub enum HooksCommand {
    /// Install git hooks in the current repository.
    Install {
        /// Overwrite existing hooks.
        #[arg(long)]
        force: bool,
    },
    /// Uninstall git hooks from the current repository.
    Uninstall,
    /// Show status of installed hooks.
    Status,
}

/// Arguments for the hooks command.
#[derive(clap::Args)]
pub struct Args {
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
                println!(
                    "  {} {}",
                    "Installed".green(),
                    hook_type.filename()
                );
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
        println!(
            "{}",
            "Use --force to overwrite existing hooks.".dimmed()
        );
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
            println!(
                "  {} {}",
                "Removed".green(),
                hook_type.filename()
            );
        }
    }

    println!();
    if removed_count > 0 {
        println!(
            "Removed {} hook(s).",
            removed_count.to_string().green()
        );
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

        println!("  {:<20} {}", format!("{}:", hook_type.filename()), status_str);
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
        fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to create hooks directory: {}", hooks_dir.display()))?;
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

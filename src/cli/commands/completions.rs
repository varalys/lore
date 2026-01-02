//! Completions command - generate and install shell completion scripts.
//!
//! Generates shell completion scripts for various shells that can be
//! installed to enable tab-completion of Lore commands and options.
//!
//! Supports two modes:
//! - Generate: Output completions to stdout for manual installation
//! - Install: Automatically install completions to the appropriate location

use anyhow::{anyhow, Context, Result};
use clap::{Command, Subcommand};
use clap_complete::{generate, Shell};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Arguments for the completions command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore completions bash              Output bash completions to stdout\n    \
    lore completions install           Auto-detect shell and install\n    \
    lore completions install --shell fish  Install fish completions\n\n\
INSTALLATION PATHS:\n    \
    Bash:       ~/.local/share/bash-completion/completions/lore\n    \
    Zsh:        ~/.zfunc/_lore\n    \
    Fish:       ~/.config/fish/completions/lore.fish\n    \
    PowerShell: Appended to $PROFILE\n    \
    Elvish:     ~/.config/elvish/lib/lore.elv")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<CompletionsCommand>,

    /// Shell to generate completions for (for backward compatibility)
    #[arg(value_name = "SHELL")]
    #[arg(value_enum)]
    #[arg(
        long_help = "The shell to generate completions for. Supported shells:\n  \
        - bash\n  \
        - zsh\n  \
        - fish\n  \
        - powershell\n  \
        - elvish"
    )]
    pub shell: Option<Shell>,
}

/// Subcommands for the completions command.
#[derive(Subcommand)]
pub enum CompletionsCommand {
    /// Install completions to the default location for your shell
    #[command(
        long_about = "Automatically detects your shell and installs completion scripts\n\
        to the appropriate location. Creates directories if needed."
    )]
    Install(InstallArgs),
}

/// Arguments for the install subcommand.
#[derive(clap::Args)]
pub struct InstallArgs {
    /// Shell to install completions for (auto-detected if not specified)
    #[arg(long, short, value_enum)]
    pub shell: Option<Shell>,
}

/// Detects the current shell from the SHELL environment variable.
///
/// Parses the basename of the SHELL path and matches it to a supported shell.
///
/// # Returns
///
/// Returns `Some(Shell)` if a supported shell is detected, `None` otherwise.
///
/// # Examples
///
/// - `/bin/zsh` -> `Some(Shell::Zsh)`
/// - `/usr/local/bin/fish` -> `Some(Shell::Fish)`
/// - `/bin/csh` -> `None` (unsupported shell)
pub fn detect_shell() -> Option<Shell> {
    let shell_path = env::var("SHELL").ok()?;
    let shell_name = PathBuf::from(&shell_path)
        .file_name()?
        .to_string_lossy()
        .to_lowercase();

    match shell_name.as_str() {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "pwsh" | "powershell" => Some(Shell::PowerShell),
        "elvish" => Some(Shell::Elvish),
        _ => None,
    }
}

/// Returns the installation path for completions for the given shell.
///
/// # Arguments
///
/// * `shell` - The shell to get the installation path for
///
/// # Returns
///
/// Returns the path where completions should be installed for the given shell.
///
/// # Shell-specific paths
///
/// - Bash: `~/.local/share/bash-completion/completions/lore`
/// - Zsh: `~/.zfunc/_lore`
/// - Fish: `~/.config/fish/completions/lore.fish`
/// - PowerShell: `$PROFILE` or platform-specific default
/// - Elvish: `~/.config/elvish/lib/lore.elv`
pub fn get_install_path(shell: Shell) -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Could not determine home directory"))?;

    let path = match shell {
        Shell::Bash => home.join(".local/share/bash-completion/completions/lore"),
        Shell::Zsh => home.join(".zfunc/_lore"),
        Shell::Fish => home.join(".config/fish/completions/lore.fish"),
        Shell::PowerShell => get_powershell_profile()?,
        Shell::Elvish => home.join(".config/elvish/lib/lore.elv"),
        _ => return Err(anyhow!("Unsupported shell for installation")),
    };

    Ok(path)
}

/// Returns the PowerShell profile path.
///
/// Tries to get the path from the PROFILE environment variable first,
/// then falls back to platform-specific defaults.
fn get_powershell_profile() -> Result<PathBuf> {
    // First try the PROFILE environment variable
    if let Ok(profile) = env::var("PROFILE") {
        return Ok(PathBuf::from(profile));
    }

    // Fall back to platform-specific defaults
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Could not determine home directory"))?;

    #[cfg(windows)]
    {
        Ok(home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1"))
    }

    #[cfg(not(windows))]
    {
        // On Unix, PowerShell uses .config/powershell
        Ok(home.join(".config/powershell/Microsoft.PowerShell_profile.ps1"))
    }
}

/// Returns a human-readable shell name.
fn shell_name(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "bash",
        Shell::Zsh => "zsh",
        Shell::Fish => "fish",
        Shell::PowerShell => "PowerShell",
        Shell::Elvish => "elvish",
        _ => "unknown",
    }
}

/// Returns shell-specific instructions for activating completions.
fn get_activation_instructions(shell: Shell, path: &Path) -> String {
    match shell {
        Shell::Bash => format!("Restart your shell or run:\n  source {}", path.display()),
        Shell::Zsh => "Restart your shell or run:\n  autoload -Uz compinit && compinit\n\n\
             Note: Ensure ~/.zfunc is in your fpath. Add to ~/.zshrc:\n  \
             fpath=(~/.zfunc $fpath)"
            .to_string(),
        Shell::Fish => format!("Restart your shell or run:\n  source {}", path.display()),
        Shell::PowerShell => format!("Restart PowerShell or run:\n  . {}", path.display()),
        Shell::Elvish => "Restart elvish or run:\n  use lore".to_string(),
        _ => "Restart your shell to activate completions.".to_string(),
    }
}

/// Generates completions and writes them to a buffer.
fn generate_completions_to_buffer(cmd: &mut Command, shell: Shell) -> Vec<u8> {
    let mut buf = Vec::new();
    generate(shell, cmd, "lore", &mut buf);
    buf
}

/// Installs completions for the specified shell.
///
/// Creates the parent directory if it does not exist, then writes the
/// completion script to the appropriate location.
///
/// # Arguments
///
/// * `cmd` - The clap Command to generate completions for
/// * `shell` - The shell to install completions for
///
/// # Returns
///
/// Returns the path where completions were installed on success.
pub fn install_completions(cmd: &mut Command, shell: Shell) -> Result<PathBuf> {
    let path = get_install_path(shell)?;

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create directory: {}\n\
                 Try creating it manually or check permissions.",
                parent.display()
            )
        })?;
    }

    // Generate completions
    let completions = generate_completions_to_buffer(cmd, shell);

    // For PowerShell, we append to the profile rather than overwrite
    if shell == Shell::PowerShell {
        // Read existing profile content
        let existing = fs::read_to_string(&path).unwrap_or_default();

        // Check if lore completions are already present
        let marker = "# Lore shell completions";
        let completions_str = String::from_utf8_lossy(&completions);

        if existing.contains(marker) {
            // Replace existing lore section
            let start_marker = "# Lore shell completions - START";
            let end_marker = "# Lore shell completions - END";

            if existing.contains(start_marker) && existing.contains(end_marker) {
                let start = existing.find(start_marker).unwrap();
                let end = existing.find(end_marker).unwrap() + end_marker.len();
                let mut new_content = String::new();
                new_content.push_str(&existing[..start]);
                new_content.push_str(start_marker);
                new_content.push('\n');
                new_content.push_str(&completions_str);
                new_content.push_str(end_marker);
                new_content.push_str(&existing[end..]);
                fs::write(&path, new_content).with_context(|| {
                    format!("Failed to write to PowerShell profile: {}", path.display())
                })?;
            }
        } else {
            // Append lore completions section
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .with_context(|| {
                    format!("Failed to open PowerShell profile: {}", path.display())
                })?;

            writeln!(file)?;
            writeln!(file, "# Lore shell completions - START")?;
            file.write_all(&completions)?;
            writeln!(file, "# Lore shell completions - END")?;
        }
    } else {
        // For other shells, just write the file (overwrite if exists)
        fs::write(&path, completions).with_context(|| {
            format!(
                "Failed to write completions to: {}\n\
                 Check permissions or try running with elevated privileges.",
                path.display()
            )
        })?;
    }

    Ok(path)
}

/// Executes the completions command.
///
/// Handles both the install subcommand and backward-compatible direct shell
/// argument for generating completions to stdout.
///
/// # Arguments
///
/// * `args` - The parsed command arguments
/// * `cmd` - The clap Command to generate completions for
pub fn run(args: Args, cmd: &mut Command) -> Result<()> {
    match args.command {
        Some(CompletionsCommand::Install(install_args)) => {
            // Determine shell (from flag or auto-detect)
            let shell = match install_args.shell {
                Some(s) => s,
                None => detect_shell().ok_or_else(|| {
                    anyhow!(
                        "Could not detect shell from $SHELL environment variable.\n\
                         Run 'lore completions install --shell <shell>' with one of:\n  \
                         bash, zsh, fish, powershell, elvish"
                    )
                })?,
            };

            println!("Detected shell: {}", shell_name(shell));

            let path = install_completions(cmd, shell)?;

            println!("Completions installed to: {}", path.display());
            println!();
            println!("{}", get_activation_instructions(shell, &path));

            Ok(())
        }
        None => {
            // Backward compatibility: lore completions <shell>
            match args.shell {
                Some(shell) => {
                    generate(shell, cmd, "lore", &mut io::stdout());
                    Ok(())
                }
                None => Err(anyhow!(
                    "Missing shell argument.\n\
                     Usage: lore completions <SHELL>\n\
                            lore completions install [--shell <SHELL>]\n\n\
                     Supported shells: bash, zsh, fish, powershell, elvish"
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify the SHELL environment variable.
    // This prevents race conditions when tests run in parallel.
    static SHELL_ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Helper to run a closure with a specific SHELL value, restoring the original afterward.
    fn with_shell_env<F, T>(shell_value: Option<&str>, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let _guard = SHELL_ENV_MUTEX.lock().unwrap();
        let original = std::env::var("SHELL").ok();

        match shell_value {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }

        let result = f();

        // Restore original
        match original {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }

        result
    }

    #[test]
    fn test_detect_shell_bash() {
        with_shell_env(Some("/bin/bash"), || {
            assert_eq!(detect_shell(), Some(Shell::Bash));
        });
        with_shell_env(Some("/usr/local/bin/bash"), || {
            assert_eq!(detect_shell(), Some(Shell::Bash));
        });
    }

    #[test]
    fn test_detect_shell_zsh() {
        with_shell_env(Some("/bin/zsh"), || {
            assert_eq!(detect_shell(), Some(Shell::Zsh));
        });
        with_shell_env(Some("/usr/local/bin/zsh"), || {
            assert_eq!(detect_shell(), Some(Shell::Zsh));
        });
    }

    #[test]
    fn test_detect_shell_fish() {
        with_shell_env(Some("/usr/bin/fish"), || {
            assert_eq!(detect_shell(), Some(Shell::Fish));
        });
        with_shell_env(Some("/opt/homebrew/bin/fish"), || {
            assert_eq!(detect_shell(), Some(Shell::Fish));
        });
    }

    #[test]
    fn test_detect_shell_powershell() {
        with_shell_env(Some("/usr/bin/pwsh"), || {
            assert_eq!(detect_shell(), Some(Shell::PowerShell));
        });
        with_shell_env(Some("/usr/local/bin/powershell"), || {
            assert_eq!(detect_shell(), Some(Shell::PowerShell));
        });
    }

    #[test]
    fn test_detect_shell_elvish() {
        with_shell_env(Some("/usr/bin/elvish"), || {
            assert_eq!(detect_shell(), Some(Shell::Elvish));
        });
    }

    #[test]
    fn test_detect_shell_unsupported() {
        with_shell_env(Some("/bin/csh"), || {
            assert_eq!(detect_shell(), None);
        });
        with_shell_env(Some("/bin/tcsh"), || {
            assert_eq!(detect_shell(), None);
        });
    }

    #[test]
    fn test_detect_shell_no_env_var() {
        with_shell_env(None, || {
            assert_eq!(detect_shell(), None);
        });
    }

    #[test]
    fn test_get_install_path_bash() {
        let path = get_install_path(Shell::Bash).unwrap();
        assert!(path.to_string_lossy().contains("bash-completion"));
        assert!(path.to_string_lossy().ends_with("lore"));
    }

    #[test]
    fn test_get_install_path_zsh() {
        let path = get_install_path(Shell::Zsh).unwrap();
        assert!(path.to_string_lossy().contains(".zfunc"));
        assert!(path.to_string_lossy().ends_with("_lore"));
    }

    #[test]
    fn test_get_install_path_fish() {
        let path = get_install_path(Shell::Fish).unwrap();
        assert!(path.to_string_lossy().contains("fish"));
        assert!(path.to_string_lossy().contains("completions"));
        assert!(path.to_string_lossy().ends_with("lore.fish"));
    }

    #[test]
    fn test_get_install_path_powershell() {
        let path = get_install_path(Shell::PowerShell).unwrap();
        assert!(
            path.to_string_lossy().contains("powershell")
                || path.to_string_lossy().contains("PowerShell")
        );
    }

    #[test]
    fn test_get_install_path_elvish() {
        let path = get_install_path(Shell::Elvish).unwrap();
        assert!(path.to_string_lossy().contains("elvish"));
        assert!(path.to_string_lossy().ends_with("lore.elv"));
    }

    #[test]
    fn test_shell_name() {
        assert_eq!(shell_name(Shell::Bash), "bash");
        assert_eq!(shell_name(Shell::Zsh), "zsh");
        assert_eq!(shell_name(Shell::Fish), "fish");
        assert_eq!(shell_name(Shell::PowerShell), "PowerShell");
        assert_eq!(shell_name(Shell::Elvish), "elvish");
    }

    #[test]
    fn test_get_activation_instructions_contains_path() {
        let path = PathBuf::from("/test/path/completions");

        let bash_instructions = get_activation_instructions(Shell::Bash, &path);
        assert!(bash_instructions.contains("source"));
        assert!(bash_instructions.contains("/test/path/completions"));

        let fish_instructions = get_activation_instructions(Shell::Fish, &path);
        assert!(fish_instructions.contains("source"));

        let zsh_instructions = get_activation_instructions(Shell::Zsh, &path);
        assert!(zsh_instructions.contains("compinit"));
        assert!(zsh_instructions.contains("fpath"));
    }

    #[test]
    fn test_generate_completions_to_buffer() {
        use clap::CommandFactory;

        // Create a minimal command for testing
        #[derive(clap::Parser)]
        struct TestCli {
            #[command(subcommand)]
            command: TestCommand,
        }

        #[derive(clap::Subcommand)]
        enum TestCommand {
            Test,
        }

        let mut cmd = TestCli::command();
        cmd = cmd.name("lore");

        let buf = generate_completions_to_buffer(&mut cmd, Shell::Bash);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("_lore"));
    }
}

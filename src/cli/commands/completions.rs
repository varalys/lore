//! Completions command - generate shell completion scripts.
//!
//! Generates shell completion scripts for various shells that can be
//! installed to enable tab-completion of Lore commands and options.

use clap::Command;
use clap_complete::{generate, Shell};
use std::io;

/// Arguments for the completions command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore completions bash > ~/.local/share/bash-completion/completions/lore\n    \
    lore completions zsh > ~/.zfunc/_lore\n    \
    lore completions fish > ~/.config/fish/completions/lore.fish\n\n\
INSTALLATION:\n    \
    Bash:       Save to ~/.local/share/bash-completion/completions/lore\n              \
    or /etc/bash_completion.d/lore\n    \
    Zsh:        Save to a directory in your $fpath (e.g., ~/.zfunc/_lore)\n              \
    and add 'autoload -Uz compinit && compinit' to ~/.zshrc\n    \
    Fish:       Save to ~/.config/fish/completions/lore.fish\n    \
    PowerShell: Add output to your $PROFILE\n    \
    Elvish:     Save to ~/.config/elvish/lib/lore.elv and use 'use lore'")]
pub struct Args {
    /// Shell to generate completions for
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
    pub shell: Shell,
}

/// Generates completions using a provided clap Command.
///
/// This should be called from main.rs which has access to the Cli struct.
///
/// # Arguments
///
/// * `cmd` - The clap Command to generate completions for
/// * `shell` - The shell to generate completions for
pub fn generate_completions(cmd: &mut Command, shell: Shell) {
    generate(shell, cmd, "lore", &mut io::stdout());
}

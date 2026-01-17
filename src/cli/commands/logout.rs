//! Logout command - remove cloud service credentials.
//!
//! Deletes stored API keys and encryption keys from the keychain
//! and any fallback credential files.

use anyhow::{Context, Result};
use colored::Colorize;

use crate::cloud::credentials::CredentialsStore;

/// Arguments for the logout command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore logout                Log out of Lore cloud")]
pub struct Args {}

/// Executes the logout command.
///
/// Removes all stored credentials and encryption keys.
pub fn run(_args: Args) -> Result<()> {
    let store = CredentialsStore::new();

    // Check if logged in
    match store.load().context("Failed to check login status")? {
        Some(creds) => {
            // Delete credentials
            store.delete().context("Failed to delete credentials")?;

            // Also delete encryption key
            if let Err(e) = store.delete_encryption_key() {
                tracing::debug!("Could not delete encryption key: {e}");
            }

            println!("Logged out from {} ({})", creds.email.cyan(), creds.plan);
        }
        None => {
            println!("{}", "Not currently logged in.".yellow());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Login/logout tests require credential storage which may not be available
    // in all test environments. The functionality is tested through integration
    // tests that can set up appropriate mocks.
}

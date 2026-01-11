//! Cline (Claude Dev) session parser.
//!
//! Parses session data from the Cline VS Code extension (formerly Claude Dev).
//! Sessions are stored in VS Code's globalStorage directory under the extension ID.
//!
//! This watcher uses the generic VS Code extension infrastructure since Cline
//! follows the standard task-based conversation storage format.

use super::vscode_extension::{VsCodeExtensionConfig, VsCodeExtensionWatcher};

/// Configuration for the Cline VS Code extension watcher.
pub const CONFIG: VsCodeExtensionConfig = VsCodeExtensionConfig {
    name: "cline",
    description: "Cline (Claude Dev) VS Code extension sessions",
    extension_id: "saoudrizwan.claude-dev",
};

/// Creates a new Cline watcher.
pub fn new_watcher() -> VsCodeExtensionWatcher {
    VsCodeExtensionWatcher::new(CONFIG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::watchers::Watcher;

    #[test]
    fn test_watcher_info() {
        let watcher = new_watcher();
        let info = watcher.info();

        assert_eq!(info.name, "cline");
        assert!(info.description.contains("Cline"));
    }

    #[test]
    fn test_watcher_extension_id() {
        assert_eq!(CONFIG.extension_id, "saoudrizwan.claude-dev");
    }
}

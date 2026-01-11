//! Roo Code session parser.
//!
//! Parses session data from the Roo Code VS Code extension (a fork of Cline).
//! Sessions are stored in VS Code's globalStorage directory under the extension ID.
//!
//! This watcher uses the generic VS Code extension infrastructure since Roo Code
//! follows the standard task-based conversation storage format.

use super::vscode_extension::{VsCodeExtensionConfig, VsCodeExtensionWatcher};

/// Configuration for the Roo Code VS Code extension watcher.
pub const CONFIG: VsCodeExtensionConfig = VsCodeExtensionConfig {
    name: "roo-code",
    description: "Roo Code VS Code extension sessions",
    extension_id: "rooveterinaryinc.roo-cline",
};

/// Creates a new Roo Code watcher.
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

        assert_eq!(info.name, "roo-code");
        assert!(info.description.contains("Roo Code"));
    }

    #[test]
    fn test_watcher_extension_id() {
        assert_eq!(CONFIG.extension_id, "rooveterinaryinc.roo-cline");
    }
}

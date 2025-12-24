//! Output formatting utilities for CLI commands.
//!
//! Provides a unified `OutputFormat` enum for consistent output formatting
//! across all CLI commands.

use clap::ValueEnum;

/// Output format options for CLI commands.
///
/// Commands can output data in different formats depending on use case:
/// - `Text` for human-readable terminal output (default)
/// - `Json` for machine-readable output and scripting
/// - `Markdown` for documentation and copy-paste to issues
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text output (default).
    #[default]
    Text,
    /// Machine-readable JSON output.
    Json,
    /// Markdown-formatted output (for show command).
    Markdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_default() {
        let format = OutputFormat::default();
        assert_eq!(format, OutputFormat::Text);
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            OutputFormat::from_str("text", false).unwrap(),
            OutputFormat::Text
        );
        assert_eq!(
            OutputFormat::from_str("json", false).unwrap(),
            OutputFormat::Json
        );
        assert_eq!(
            OutputFormat::from_str("markdown", false).unwrap(),
            OutputFormat::Markdown
        );
    }
}

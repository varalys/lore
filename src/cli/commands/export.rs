//! Export command - export sessions in various formats with optional redaction.
//!
//! Exports session data as markdown or JSON, with support for redacting
//! sensitive information like API keys, tokens, passwords, and email addresses.

use anyhow::Result;
use regex::Regex;
use serde::Serialize;

use crate::storage::{ContentBlock, Database, Message, MessageContent, MessageRole, Session};

/// Arguments for the export command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore export abc123                     Export as markdown (default)\n    \
    lore export abc123 --format json       Export as JSON\n    \
    lore export abc123 --redact            Redact sensitive data\n    \
    lore export abc123 --redact-pattern 'secret_\\w+'  Custom redaction")]
pub struct Args {
    /// Session ID prefix to export
    #[arg(value_name = "SESSION")]
    #[arg(long_help = "The session ID prefix to export. You only need to\n\
        provide enough characters to uniquely identify the session.")]
    pub session: String,

    /// Output format: markdown (default) or json
    #[arg(short, long, value_enum, default_value = "markdown")]
    pub format: ExportFormat,

    /// Redact sensitive data (API keys, tokens, passwords, emails, IPs)
    #[arg(long)]
    #[arg(long_help = "Redact common sensitive patterns from the output:\n\
        - API keys and tokens (Bearer, sk-, api_key, etc.)\n\
        - Passwords and secrets\n\
        - Email addresses\n\
        - IP addresses")]
    pub redact: bool,

    /// Additional regex pattern to redact (can be specified multiple times)
    #[arg(long = "redact-pattern", value_name = "REGEX")]
    #[arg(long_help = "Custom regex pattern to redact from the output.\n\
        Can be specified multiple times for multiple patterns.\n\
        Example: --redact-pattern 'secret_\\w+'")]
    pub redact_patterns: Vec<String>,

    /// Write output to a file instead of stdout
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<String>,
}

/// Export format options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    /// Human-readable markdown format (default).
    #[default]
    Markdown,
    /// Machine-readable JSON format.
    Json,
}

/// JSON export structure for a complete session.
#[derive(Serialize)]
struct ExportedSession {
    session: SessionMetadata,
    messages: Vec<ExportedMessage>,
    links: Vec<ExportedLink>,
    tags: Vec<String>,
    summary: Option<String>,
}

/// Session metadata for export.
#[derive(Serialize)]
struct SessionMetadata {
    id: String,
    tool: String,
    tool_version: Option<String>,
    model: Option<String>,
    started_at: String,
    ended_at: Option<String>,
    duration_minutes: Option<i64>,
    working_directory: String,
    git_branch: Option<String>,
    message_count: i32,
}

/// Exported message structure.
#[derive(Serialize)]
struct ExportedMessage {
    index: i32,
    timestamp: String,
    role: String,
    content: String,
}

/// Exported link structure.
#[derive(Serialize)]
struct ExportedLink {
    commit_sha: Option<String>,
    branch: Option<String>,
    confidence: Option<f64>,
    created_at: String,
}

/// Executes the export command.
///
/// Exports a session in the specified format with optional redaction.
pub fn run(args: Args) -> Result<()> {
    let db = Database::open_default()?;

    // Find the session
    let session = match db.find_session_by_id_prefix(&args.session)? {
        Some(s) => s,
        None => {
            if db.session_count()? == 0 {
                anyhow::bail!(
                    "No session found matching '{}'. No sessions in database. \
                     Run 'lore import' to import sessions first.",
                    args.session
                );
            } else {
                anyhow::bail!(
                    "No session found matching '{}'. \
                     Run 'lore sessions' to list available sessions.",
                    args.session
                );
            }
        }
    };

    // Build the redactor
    let redactor = Redactor::new(args.redact, &args.redact_patterns)?;

    // Get related data
    let messages = db.get_messages(&session.id)?;
    let links = db.get_links_by_session(&session.id)?;
    let tags = db.get_tags(&session.id)?;
    let summary = db.get_summary(&session.id)?;

    // Generate output
    let output = match args.format {
        ExportFormat::Markdown => {
            export_markdown(&session, &messages, &links, &tags, &summary, &redactor)
        }
        ExportFormat::Json => export_json(&session, &messages, &links, &tags, &summary, &redactor)?,
    };

    // Write output
    if let Some(path) = args.output {
        std::fs::write(&path, &output)
            .map_err(|e| anyhow::anyhow!("Failed to write to {}: {}", path, e))?;
        eprintln!("Exported session to: {path}");
    } else {
        println!("{output}");
    }

    Ok(())
}

/// Handles redaction of sensitive data.
struct Redactor {
    patterns: Vec<Regex>,
}

impl Redactor {
    /// Creates a new redactor with built-in and custom patterns.
    fn new(use_builtin: bool, custom_patterns: &[String]) -> Result<Self> {
        let mut patterns = Vec::new();

        if use_builtin {
            // API keys and tokens
            patterns.push(Regex::new(
                r#"(?i)(api[_-]?key|apikey|api_secret)[=:\s]+['"]?[\w\-]{16,}['"]?"#,
            )?);
            patterns.push(Regex::new(
                r#"(?i)(access[_-]?token|auth[_-]?token|bearer)[=:\s]+['"]?[\w\-\.]{16,}['"]?"#,
            )?);
            patterns.push(Regex::new(r"(?i)Bearer\s+[\w\-\.]{16,}")?);
            patterns.push(Regex::new(r"sk-[a-zA-Z0-9]{20,}")?); // OpenAI-style keys
            patterns.push(Regex::new(
                r#"(?i)(secret|password|passwd|pwd)[=:\s]+['"]?[^\s'"]{8,}['"]?"#,
            )?);

            // AWS keys
            patterns.push(Regex::new(r"AKIA[0-9A-Z]{16}")?);
            patterns.push(Regex::new(
                r#"(?i)aws[_-]?secret[_-]?access[_-]?key[=:\s]+['"]?[\w/\+]{40}['"]?"#,
            )?);

            // GitHub tokens
            patterns.push(Regex::new(r"ghp_[a-zA-Z0-9]{36}")?);
            patterns.push(Regex::new(r"gho_[a-zA-Z0-9]{36}")?);
            patterns.push(Regex::new(r"ghu_[a-zA-Z0-9]{36}")?);
            patterns.push(Regex::new(r"ghs_[a-zA-Z0-9]{36}")?);
            patterns.push(Regex::new(r"ghr_[a-zA-Z0-9]{36}")?);

            // Email addresses
            patterns.push(Regex::new(
                r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}",
            )?);

            // IP addresses (IPv4)
            patterns.push(Regex::new(r"\b(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\b")?);

            // Private keys
            patterns.push(Regex::new(
                r"-----BEGIN [A-Z ]+ PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+ PRIVATE KEY-----",
            )?);

            // Connection strings
            patterns.push(Regex::new(
                r"(?i)(mysql|postgres|postgresql|mongodb|redis)://[^\s]+",
            )?);
        }

        // Add custom patterns
        for pattern in custom_patterns {
            patterns.push(
                Regex::new(pattern)
                    .map_err(|e| anyhow::anyhow!("Invalid regex pattern '{}': {}", pattern, e))?,
            );
        }

        Ok(Self { patterns })
    }

    /// Redacts sensitive data from the given text.
    fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for pattern in &self.patterns {
            result = pattern.replace_all(&result, "[REDACTED]").to_string();
        }
        result
    }
}

/// Exports a session as markdown.
fn export_markdown(
    session: &Session,
    messages: &[Message],
    links: &[crate::storage::SessionLink],
    tags: &[crate::storage::Tag],
    summary: &Option<crate::storage::Summary>,
    redactor: &Redactor,
) -> String {
    let mut output = String::new();

    // Header
    output.push_str(&format!("# Session {}\n\n", session.id));

    // Metadata
    output.push_str("## Metadata\n\n");
    output.push_str("| Property | Value |\n");
    output.push_str("|----------|-------|\n");
    output.push_str(&format!("| Tool | {} |\n", session.tool));
    if let Some(ref v) = session.tool_version {
        output.push_str(&format!("| Version | {v} |\n"));
    }
    if let Some(ref m) = session.model {
        output.push_str(&format!("| Model | {m} |\n"));
    }
    output.push_str(&format!(
        "| Started | {} |\n",
        session.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    if let Some(ended) = session.ended_at {
        let duration = ended.signed_duration_since(session.started_at);
        output.push_str(&format!(
            "| Ended | {} |\n",
            ended.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        output.push_str(&format!(
            "| Duration | {} minutes |\n",
            duration.num_minutes()
        ));
    }
    output.push_str(&format!("| Messages | {} |\n", session.message_count));
    output.push_str(&format!(
        "| Directory | `{}` |\n",
        redactor.redact(&session.working_directory)
    ));
    if let Some(ref branch) = session.git_branch {
        output.push_str(&format!("| Branch | `{branch}` |\n"));
    }
    output.push('\n');

    // Tags
    if !tags.is_empty() {
        output.push_str("## Tags\n\n");
        for tag in tags {
            output.push_str(&format!("- {}\n", tag.label));
        }
        output.push('\n');
    }

    // Summary
    if let Some(ref s) = summary {
        output.push_str("## Summary\n\n");
        output.push_str(&redactor.redact(&s.content));
        output.push_str("\n\n");
    }

    // Linked commits
    if !links.is_empty() {
        output.push_str("## Linked Commits\n\n");
        for link in links {
            if let Some(ref sha) = link.commit_sha {
                let short_sha = &sha[..8.min(sha.len())];
                output.push_str(&format!("- `{short_sha}`"));
                if let Some(ref branch) = link.branch {
                    output.push_str(&format!(" on `{branch}`"));
                }
                if let Some(conf) = link.confidence {
                    output.push_str(&format!(" (confidence: {:.0}%)", conf * 100.0));
                }
                output.push('\n');
            }
        }
        output.push('\n');
    }

    // Conversation
    output.push_str("## Conversation\n\n");

    for msg in messages {
        let role = match msg.role {
            MessageRole::User => "Human",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
        };

        let time = msg.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
        output.push_str(&format!("### [{role}] {time}\n\n"));

        let content = format_message_content_markdown(&msg.content, redactor);
        output.push_str(&content);
        output.push_str("\n\n");
    }

    output
}

/// Formats message content for markdown export.
fn format_message_content_markdown(content: &MessageContent, redactor: &Redactor) -> String {
    match content {
        MessageContent::Text(text) => redactor.redact(text),
        MessageContent::Blocks(blocks) => {
            let mut output = String::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        output.push_str(&redactor.redact(text));
                        output.push('\n');
                    }
                    ContentBlock::Thinking { thinking } => {
                        output.push_str("<details>\n<summary>Thinking</summary>\n\n");
                        output.push_str(&redactor.redact(thinking));
                        output.push_str("\n\n</details>\n\n");
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        output.push_str(&format!("**Tool: {name}**\n\n"));
                        output.push_str("```json\n");
                        let json = serde_json::to_string_pretty(input).unwrap_or_default();
                        output.push_str(&redactor.redact(&json));
                        output.push_str("\n```\n\n");
                    }
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        let label = if *is_error { "Error" } else { "Result" };
                        output.push_str(&format!("**{label}:**\n\n"));
                        output.push_str("```\n");
                        output.push_str(&redactor.redact(content));
                        output.push_str("\n```\n\n");
                    }
                }
            }
            output
        }
    }
}

/// Exports a session as JSON.
fn export_json(
    session: &Session,
    messages: &[Message],
    links: &[crate::storage::SessionLink],
    tags: &[crate::storage::Tag],
    summary: &Option<crate::storage::Summary>,
    redactor: &Redactor,
) -> Result<String> {
    let duration = session.ended_at.map(|ended| {
        ended
            .signed_duration_since(session.started_at)
            .num_minutes()
    });

    let exported = ExportedSession {
        session: SessionMetadata {
            id: session.id.to_string(),
            tool: session.tool.clone(),
            tool_version: session.tool_version.clone(),
            model: session.model.clone(),
            started_at: session.started_at.to_rfc3339(),
            ended_at: session.ended_at.map(|t| t.to_rfc3339()),
            duration_minutes: duration,
            working_directory: redactor.redact(&session.working_directory),
            git_branch: session.git_branch.clone(),
            message_count: session.message_count,
        },
        messages: messages
            .iter()
            .map(|m| ExportedMessage {
                index: m.index,
                timestamp: m.timestamp.to_rfc3339(),
                role: m.role.to_string(),
                content: redactor.redact(&format_message_content_plain(&m.content)),
            })
            .collect(),
        links: links
            .iter()
            .map(|l| ExportedLink {
                commit_sha: l.commit_sha.clone(),
                branch: l.branch.clone(),
                confidence: l.confidence,
                created_at: l.created_at.to_rfc3339(),
            })
            .collect(),
        tags: tags.iter().map(|t| t.label.clone()).collect(),
        summary: summary.as_ref().map(|s| redactor.redact(&s.content)),
    };

    let json = serde_json::to_string_pretty(&exported)?;
    Ok(json)
}

/// Formats message content as plain text for JSON export.
fn format_message_content_plain(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Blocks(blocks) => {
            let mut output = String::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        output.push_str(text);
                        output.push('\n');
                    }
                    ContentBlock::Thinking { thinking } => {
                        output.push_str("[Thinking]\n");
                        output.push_str(thinking);
                        output.push('\n');
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        output.push_str(&format!("[Tool: {name}]\n"));
                        output.push_str(&serde_json::to_string(input).unwrap_or_default());
                        output.push('\n');
                    }
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        let label = if *is_error { "Error" } else { "Result" };
                        output.push_str(&format!("[{label}]\n"));
                        output.push_str(content);
                        output.push('\n');
                    }
                }
            }
            output
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redactor_empty() {
        let redactor = Redactor::new(false, &[]).unwrap();
        let text = "This text has an api_key=secret123456789012 in it.";
        // Without builtin patterns, nothing should be redacted
        assert_eq!(redactor.redact(text), text);
    }

    #[test]
    fn test_redactor_api_key() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "Config: api_key=sk-abcdefghij1234567890";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("sk-abcdefghij1234567890"));
    }

    #[test]
    fn test_redactor_bearer_token() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWI";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redactor_email() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "Contact: user@example.com for support.";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("user@example.com"));
    }

    #[test]
    fn test_redactor_ip_address() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "Server IP: 192.168.1.100";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("192.168.1.100"));
    }

    #[test]
    fn test_redactor_openai_key() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "OPENAI_API_KEY=sk-abc123def456ghi789jkl012mno345pqr678stu901vwx";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redactor_github_token() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "GITHUB_TOKEN=ghp_abcdefghij1234567890abcdefghij123456";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redactor_aws_key() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redactor_custom_pattern() {
        let redactor = Redactor::new(false, &["secret_\\w+".to_string()]).unwrap();
        let text = "The secret_value123 should be redacted.";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("secret_value123"));
    }

    #[test]
    fn test_redactor_multiple_custom_patterns() {
        let patterns = vec!["foo\\d+".to_string(), "bar\\d+".to_string()];
        let redactor = Redactor::new(false, &patterns).unwrap();
        let text = "Values: foo123 and bar456 should be redacted.";
        let redacted = redactor.redact(text);
        assert!(!redacted.contains("foo123"));
        assert!(!redacted.contains("bar456"));
    }

    #[test]
    fn test_redactor_invalid_pattern() {
        let result = Redactor::new(false, &["[invalid".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_redactor_private_key() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA1234567890
-----END RSA PRIVATE KEY-----"#;
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redactor_connection_string() {
        let redactor = Redactor::new(true, &[]).unwrap();
        let text = "DATABASE_URL=postgres://user:pass@localhost:5432/db";
        let redacted = redactor.redact(text);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_format_message_content_plain_text() {
        let content = MessageContent::Text("Hello, world!".to_string());
        let result = format_message_content_plain(&content);
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_format_message_content_plain_blocks() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "Line 1".to_string(),
            },
            ContentBlock::Text {
                text: "Line 2".to_string(),
            },
        ]);
        let result = format_message_content_plain(&content);
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn test_export_format_default() {
        let format = ExportFormat::default();
        assert_eq!(format, ExportFormat::Markdown);
    }
}

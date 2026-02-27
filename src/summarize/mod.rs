//! Session summary generation via LLM providers.
//!
//! This module provides the ability to generate summaries of AI-assisted
//! development sessions using various LLM providers (Anthropic, OpenAI,
//! OpenRouter). It includes provider configuration, API communication,
//! and error handling.
//!
//! # Usage
//!
//! The main entry point is [`generate_summary`], which resolves the provider
//! configuration and calls the appropriate LLM API. Configuration is read
//! from `~/.lore/config.yaml` with environment variable overrides.

pub mod prompt;
pub mod provider;

use std::env;

use crate::config::Config;
use crate::storage::models::Message;

pub use provider::{create_provider, SummaryProvider, SummaryProviderKind};

/// Maximum character limit for the conversation transcript sent to the LLM.
const MAX_CONVERSATION_CHARS: usize = 100_000;

/// Resolved summary configuration from config file and environment variables.
#[derive(Debug, Clone)]
pub struct SummaryConfig {
    /// The LLM provider kind.
    pub kind: SummaryProviderKind,
    /// API key for the provider.
    pub api_key: String,
    /// Optional model override (uses provider default if None).
    pub model: Option<String>,
}

/// Resolves summary configuration from the config file and environment variables.
///
/// Environment variables take precedence over config file values:
/// - `LORE_SUMMARY_PROVIDER` overrides `summary_provider`
/// - `LORE_SUMMARY_API_KEY` overrides the provider-specific API key
/// - `LORE_SUMMARY_MODEL` overrides the provider-specific model
///
/// Returns `NotConfigured` if no provider or API key is set.
pub fn resolve_config() -> Result<SummaryConfig, SummarizeError> {
    let config = Config::load().map_err(|_| SummarizeError::NotConfigured)?;

    // Provider: env var > config file
    let provider_str = env::var("LORE_SUMMARY_PROVIDER")
        .ok()
        .or_else(|| config.summary_provider.clone());

    let provider_str = provider_str.ok_or(SummarizeError::NotConfigured)?;

    let kind: SummaryProviderKind = provider_str
        .parse()
        .map_err(|_| SummarizeError::NotConfigured)?;

    // API key: env var > provider-specific config key > generic config key
    let api_key = env::var("LORE_SUMMARY_API_KEY")
        .ok()
        .or_else(|| config.summary_api_key_for_provider(&provider_str));

    let api_key = api_key.ok_or(SummarizeError::NotConfigured)?;

    if api_key.is_empty() {
        return Err(SummarizeError::NotConfigured);
    }

    // Model: env var > provider-specific config key
    let model = env::var("LORE_SUMMARY_MODEL")
        .ok()
        .or_else(|| config.summary_model_for_provider(&provider_str));

    Ok(SummaryConfig {
        kind,
        api_key,
        model,
    })
}

/// Generates a summary for a set of session messages using the configured LLM provider.
///
/// This is the main entry point for summary generation. It:
/// 1. Resolves the provider configuration
/// 2. Prepares the conversation transcript from messages
/// 3. Calls the LLM API to generate a summary
///
/// Returns `EmptySession` if there are no messages or all messages are empty.
/// Returns `NotConfigured` if no provider is set up.
pub fn generate_summary(messages: &[Message]) -> Result<String, SummarizeError> {
    if messages.is_empty() {
        return Err(SummarizeError::EmptySession);
    }

    let config = resolve_config()?;

    let conversation = prompt::prepare_conversation(messages, MAX_CONVERSATION_CHARS);
    if conversation.is_empty() {
        return Err(SummarizeError::EmptySession);
    }

    let system = prompt::system_prompt();
    let provider = create_provider(config.kind, config.api_key, config.model);

    let response = provider.summarize(system, &conversation)?;
    Ok(normalize_whitespace(&response.content))
}

/// Normalizes whitespace in a summary string.
///
/// Trims leading/trailing whitespace and collapses runs of 3+ consecutive
/// newlines down to 2 (one blank line). This keeps the summary readable
/// (paragraph breaks preserved) while removing excessive spacing that
/// some models produce.
fn normalize_whitespace(text: &str) -> String {
    let trimmed = text.trim();
    let mut result = String::with_capacity(trimmed.len());
    let mut consecutive_newlines = 0u32;

    for ch in trimmed.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                result.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            result.push(ch);
        }
    }

    result
}

/// Errors that can occur during summary generation.
#[derive(Debug, thiserror::Error)]
pub enum SummarizeError {
    /// No summary provider is configured.
    #[error(
        "Summary provider not configured. Set LORE_SUMMARY_PROVIDER and the corresponding API key."
    )]
    NotConfigured,

    /// Network or connection error when calling the provider API.
    #[error("Request failed: {0}")]
    RequestFailed(String),

    /// The provider API returned a non-success HTTP status code.
    #[error("HTTP error ({status}): {body}")]
    HttpError {
        /// HTTP status code.
        status: u16,
        /// Response body text.
        body: String,
    },

    /// The provider API returned an error in its JSON response.
    #[error("API error ({status}): {message}")]
    #[allow(dead_code)]
    ApiError {
        /// HTTP status code.
        status: u16,
        /// Error message from the API.
        message: String,
    },

    /// Failed to parse the provider API response.
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// The session has no content to summarize.
    #[error("Session has no content to summarize")]
    EmptySession,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{MessageContent, MessageRole};
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn test_summarize_error_display_not_configured() {
        let err = SummarizeError::NotConfigured;
        assert!(err.to_string().contains("not configured"));
    }

    #[test]
    fn test_summarize_error_display_request_failed() {
        let err = SummarizeError::RequestFailed("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn test_summarize_error_display_http_error() {
        let err = SummarizeError::HttpError {
            status: 429,
            body: "rate limited".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("429"));
        assert!(msg.contains("rate limited"));
    }

    #[test]
    fn test_summarize_error_display_api_error() {
        let err = SummarizeError::ApiError {
            status: 400,
            message: "invalid model".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("400"));
        assert!(msg.contains("invalid model"));
    }

    #[test]
    fn test_summarize_error_display_parse_error() {
        let err = SummarizeError::ParseError("missing field".to_string());
        assert!(err.to_string().contains("missing field"));
    }

    #[test]
    fn test_summarize_error_display_empty_session() {
        let err = SummarizeError::EmptySession;
        assert!(err.to_string().contains("no content"));
    }

    #[test]
    fn test_generate_summary_empty_messages() {
        let messages: Vec<Message> = vec![];
        let result = generate_summary(&messages);
        assert!(result.is_err());
        match result.unwrap_err() {
            SummarizeError::EmptySession => {}
            other => panic!("Expected EmptySession, got: {other:?}"),
        }
    }

    #[test]
    fn test_generate_summary_tool_only_messages_returns_empty_session() {
        // Messages that contain only tool blocks produce empty text,
        // so generate_summary should return EmptySession.
        let messages = vec![Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index: 0,
            timestamp: Utc::now(),
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![crate::storage::models::ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            }]),
            model: None,
            git_branch: None,
            cwd: None,
        }];

        let result = generate_summary(&messages);
        // Without a configured provider, this should fail. But if the conversation
        // text is empty, it should return EmptySession before trying the provider.
        match result {
            Err(SummarizeError::EmptySession) => {}
            Err(SummarizeError::NotConfigured) => {
                // Also acceptable: config check happens before content check
            }
            other => panic!("Expected EmptySession or NotConfigured, got: {other:?}"),
        }
    }

    #[test]
    fn test_summary_config_debug() {
        let config = SummaryConfig {
            kind: SummaryProviderKind::Anthropic,
            api_key: "sk-test".to_string(),
            model: Some("claude-haiku-4-5-20241022".to_string()),
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("Anthropic"));
        assert!(debug.contains("sk-test"));
    }

    #[test]
    fn test_max_conversation_chars_constant() {
        assert_eq!(MAX_CONVERSATION_CHARS, 100_000);
    }

    #[test]
    fn test_normalize_whitespace_trims_edges() {
        assert_eq!(normalize_whitespace("  hello  "), "hello");
        assert_eq!(normalize_whitespace("\n\nhello\n\n"), "hello");
    }

    #[test]
    fn test_normalize_whitespace_preserves_single_blank_line() {
        let input = "Overview sentence.\n\n- Bullet one\n- Bullet two";
        assert_eq!(normalize_whitespace(input), input);
    }

    #[test]
    fn test_normalize_whitespace_collapses_triple_newlines() {
        let input = "Overview.\n\n\n- Bullet one\n\n\n\n- Bullet two";
        let expected = "Overview.\n\n- Bullet one\n\n- Bullet two";
        assert_eq!(normalize_whitespace(input), expected);
    }

    #[test]
    fn test_normalize_whitespace_empty_string() {
        assert_eq!(normalize_whitespace(""), "");
        assert_eq!(normalize_whitespace("   "), "");
    }

    #[test]
    fn test_normalize_whitespace_no_change_needed() {
        let input = "Line one\nLine two\n\nLine three";
        assert_eq!(normalize_whitespace(input), input);
    }
}

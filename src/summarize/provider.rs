//! LLM provider integrations for session summary generation.
//!
//! Supports Anthropic, OpenAI, and OpenRouter as summary providers.
//! Each provider implements the [`SummaryProvider`] trait, and the
//! [`create_provider`] factory builds the appropriate provider from
//! configuration.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

use super::SummarizeError;

/// Timeout for establishing a connection (30 seconds).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for the entire request including response (120 seconds).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

// ==================== Types ====================

/// Supported LLM provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryProviderKind {
    /// Anthropic Claude API.
    Anthropic,
    /// OpenAI ChatGPT API.
    OpenAI,
    /// OpenRouter unified API.
    OpenRouter,
}

impl fmt::Display for SummaryProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SummaryProviderKind::Anthropic => write!(f, "anthropic"),
            SummaryProviderKind::OpenAI => write!(f, "openai"),
            SummaryProviderKind::OpenRouter => write!(f, "openrouter"),
        }
    }
}

impl FromStr for SummaryProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(SummaryProviderKind::Anthropic),
            "openai" => Ok(SummaryProviderKind::OpenAI),
            "openrouter" => Ok(SummaryProviderKind::OpenRouter),
            other => Err(format!("Unknown summary provider: '{other}'. Expected one of: anthropic, openai, openrouter")),
        }
    }
}

/// Response from an LLM summary request.
#[derive(Debug, Clone)]
pub struct SummaryResponse {
    /// The generated summary text.
    pub content: String,
}

// ==================== Trait ====================

/// Trait for LLM providers that can generate summaries.
///
/// Implementors send a system prompt and user content to an LLM API
/// and return the generated summary text.
pub trait SummaryProvider {
    /// Generate a summary from the given prompts.
    ///
    /// The system prompt provides instructions for how to summarize,
    /// while the user content contains the session data to summarize.
    fn summarize(
        &self,
        system_prompt: &str,
        user_content: &str,
    ) -> Result<SummaryResponse, SummarizeError>;
}

// ==================== Anthropic ====================

/// Anthropic Claude API provider.
pub(crate) struct AnthropicProvider {
    /// HTTP client instance.
    client: Client,
    /// Anthropic API key.
    api_key: String,
    /// Model identifier (e.g., "claude-haiku-4-5").
    model: String,
}

impl AnthropicProvider {
    /// Creates a new Anthropic provider.
    pub(crate) fn new(client: Client, api_key: String, model: String) -> Self {
        Self {
            client,
            api_key,
            model,
        }
    }

    /// Builds the JSON request body for the Anthropic Messages API.
    fn build_request_body(&self, system_prompt: &str, user_content: &str) -> Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "system": system_prompt,
            "messages": [
                {
                    "role": "user",
                    "content": user_content,
                }
            ]
        })
    }
}

impl SummaryProvider for AnthropicProvider {
    fn summarize(
        &self,
        system_prompt: &str,
        user_content: &str,
    ) -> Result<SummaryResponse, SummarizeError> {
        let body = self.build_request_body(system_prompt, user_content);

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| SummarizeError::RequestFailed(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(SummarizeError::HttpError {
                status: status_code,
                body: body_text,
            });
        }

        let json: Value = response
            .json()
            .map_err(|e| SummarizeError::ParseError(e.to_string()))?;

        let content = json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                SummarizeError::ParseError(
                    "Missing content[0].text in Anthropic response".to_string(),
                )
            })?;

        Ok(SummaryResponse {
            content: content.to_string(),
        })
    }
}

// ==================== OpenAI ====================

/// OpenAI ChatGPT API provider.
pub(crate) struct OpenAIProvider {
    /// HTTP client instance.
    client: Client,
    /// OpenAI API key.
    api_key: String,
    /// Model identifier (e.g., "gpt-4o-mini").
    model: String,
}

impl OpenAIProvider {
    /// Creates a new OpenAI provider.
    pub(crate) fn new(client: Client, api_key: String, model: String) -> Self {
        Self {
            client,
            api_key,
            model,
        }
    }

    /// Builds the JSON request body for the OpenAI Chat Completions API.
    fn build_request_body(&self, system_prompt: &str, user_content: &str) -> Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "system",
                    "content": system_prompt,
                },
                {
                    "role": "user",
                    "content": user_content,
                }
            ]
        })
    }
}

impl SummaryProvider for OpenAIProvider {
    fn summarize(
        &self,
        system_prompt: &str,
        user_content: &str,
    ) -> Result<SummaryResponse, SummarizeError> {
        let body = self.build_request_body(system_prompt, user_content);

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| SummarizeError::RequestFailed(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(SummarizeError::HttpError {
                status: status_code,
                body: body_text,
            });
        }

        let json: Value = response
            .json()
            .map_err(|e| SummarizeError::ParseError(e.to_string()))?;

        parse_openai_response(&json)
    }
}

// ==================== OpenRouter ====================

/// OpenRouter unified API provider.
///
/// Uses the same request/response format as OpenAI but with a different
/// base URL and an additional HTTP-Referer header.
pub(crate) struct OpenRouterProvider {
    /// HTTP client instance.
    client: Client,
    /// OpenRouter API key.
    api_key: String,
    /// Model identifier (e.g., "meta-llama/llama-3.1-8b-instruct:free").
    model: String,
}

impl OpenRouterProvider {
    /// Creates a new OpenRouter provider.
    pub(crate) fn new(client: Client, api_key: String, model: String) -> Self {
        Self {
            client,
            api_key,
            model,
        }
    }

    /// Builds the JSON request body for the OpenRouter API.
    ///
    /// Uses the same format as OpenAI Chat Completions.
    fn build_request_body(&self, system_prompt: &str, user_content: &str) -> Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "system",
                    "content": system_prompt,
                },
                {
                    "role": "user",
                    "content": user_content,
                }
            ]
        })
    }
}

impl SummaryProvider for OpenRouterProvider {
    fn summarize(
        &self,
        system_prompt: &str,
        user_content: &str,
    ) -> Result<SummaryResponse, SummarizeError> {
        let body = self.build_request_body(system_prompt, user_content);

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://lore.varalys.com")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| SummarizeError::RequestFailed(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(SummarizeError::HttpError {
                status: status_code,
                body: body_text,
            });
        }

        let json: Value = response
            .json()
            .map_err(|e| SummarizeError::ParseError(e.to_string()))?;

        parse_openai_response(&json)
    }
}

// ==================== Shared Helpers ====================

/// Parses a response in the OpenAI Chat Completions format.
///
/// Extracts `choices[0].message.content` from the JSON response.
/// Used by both OpenAI and OpenRouter providers.
fn parse_openai_response(json: &Value) -> Result<SummaryResponse, SummarizeError> {
    let content = json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|msg| msg.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            SummarizeError::ParseError("Missing choices[0].message.content in response".to_string())
        })?;

    Ok(SummaryResponse {
        content: content.to_string(),
    })
}

// ==================== Factory ====================

/// Returns the default model for the given provider kind.
pub fn default_model(kind: SummaryProviderKind) -> &'static str {
    match kind {
        SummaryProviderKind::Anthropic => "claude-haiku-4-5",
        SummaryProviderKind::OpenAI => "gpt-4o-mini",
        SummaryProviderKind::OpenRouter => "meta-llama/llama-3.1-8b-instruct:free",
    }
}

/// Creates a summary provider for the given kind.
///
/// If `model` is `None`, uses the default model for the provider kind.
/// The returned provider is ready to make API calls.
pub fn create_provider(
    kind: SummaryProviderKind,
    api_key: String,
    model: Option<String>,
) -> Box<dyn SummaryProvider> {
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("Failed to build HTTP client");

    let model = model.unwrap_or_else(|| default_model(kind).to_string());

    match kind {
        SummaryProviderKind::Anthropic => Box::new(AnthropicProvider::new(client, api_key, model)),
        SummaryProviderKind::OpenAI => Box::new(OpenAIProvider::new(client, api_key, model)),
        SummaryProviderKind::OpenRouter => {
            Box::new(OpenRouterProvider::new(client, api_key, model))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an HTTP client for use in tests.
    fn build_client() -> Client {
        Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("Failed to build HTTP client")
    }

    // ==================== default_model tests ====================

    #[test]
    fn test_default_model_anthropic() {
        assert_eq!(
            default_model(SummaryProviderKind::Anthropic),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn test_default_model_openai() {
        assert_eq!(default_model(SummaryProviderKind::OpenAI), "gpt-4o-mini");
    }

    #[test]
    fn test_default_model_openrouter() {
        assert_eq!(
            default_model(SummaryProviderKind::OpenRouter),
            "meta-llama/llama-3.1-8b-instruct:free"
        );
    }

    // ==================== SummaryProviderKind Display tests ====================

    #[test]
    fn test_provider_kind_display_anthropic() {
        assert_eq!(SummaryProviderKind::Anthropic.to_string(), "anthropic");
    }

    #[test]
    fn test_provider_kind_display_openai() {
        assert_eq!(SummaryProviderKind::OpenAI.to_string(), "openai");
    }

    #[test]
    fn test_provider_kind_display_openrouter() {
        assert_eq!(SummaryProviderKind::OpenRouter.to_string(), "openrouter");
    }

    // ==================== SummaryProviderKind FromStr tests ====================

    #[test]
    fn test_provider_kind_from_str_anthropic() {
        assert_eq!(
            SummaryProviderKind::from_str("anthropic").unwrap(),
            SummaryProviderKind::Anthropic
        );
    }

    #[test]
    fn test_provider_kind_from_str_openai() {
        assert_eq!(
            SummaryProviderKind::from_str("openai").unwrap(),
            SummaryProviderKind::OpenAI
        );
    }

    #[test]
    fn test_provider_kind_from_str_openrouter() {
        assert_eq!(
            SummaryProviderKind::from_str("openrouter").unwrap(),
            SummaryProviderKind::OpenRouter
        );
    }

    #[test]
    fn test_provider_kind_from_str_case_insensitive() {
        assert_eq!(
            SummaryProviderKind::from_str("ANTHROPIC").unwrap(),
            SummaryProviderKind::Anthropic
        );
        assert_eq!(
            SummaryProviderKind::from_str("OpenAI").unwrap(),
            SummaryProviderKind::OpenAI
        );
        assert_eq!(
            SummaryProviderKind::from_str("OpenRouter").unwrap(),
            SummaryProviderKind::OpenRouter
        );
    }

    #[test]
    fn test_provider_kind_from_str_unknown() {
        let err = SummaryProviderKind::from_str("gemini").unwrap_err();
        assert!(err.contains("Unknown summary provider"));
        assert!(err.contains("gemini"));
    }

    // ==================== create_provider tests ====================

    #[test]
    fn test_create_provider_anthropic_does_not_panic() {
        let _provider =
            create_provider(SummaryProviderKind::Anthropic, "test-key".to_string(), None);
    }

    #[test]
    fn test_create_provider_openai_does_not_panic() {
        let _provider = create_provider(SummaryProviderKind::OpenAI, "test-key".to_string(), None);
    }

    #[test]
    fn test_create_provider_openrouter_does_not_panic() {
        let _provider = create_provider(
            SummaryProviderKind::OpenRouter,
            "test-key".to_string(),
            None,
        );
    }

    #[test]
    fn test_create_provider_with_custom_model() {
        let _provider = create_provider(
            SummaryProviderKind::Anthropic,
            "test-key".to_string(),
            Some("claude-sonnet-4-20250514".to_string()),
        );
    }

    // ==================== Request body construction tests ====================

    #[test]
    fn test_anthropic_request_body() {
        let provider = AnthropicProvider::new(
            build_client(),
            "test-key".to_string(),
            "claude-haiku-4-5".to_string(),
        );

        let body = provider.build_request_body("Be concise.", "Summarize this session.");

        assert_eq!(body["model"], "claude-haiku-4-5");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["system"], "Be concise.");

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Summarize this session.");
    }

    #[test]
    fn test_openai_request_body() {
        let provider = OpenAIProvider::new(
            build_client(),
            "test-key".to_string(),
            "gpt-4o-mini".to_string(),
        );

        let body = provider.build_request_body("Be concise.", "Summarize this session.");

        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["max_tokens"], 1024);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "Be concise.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Summarize this session.");
    }

    #[test]
    fn test_openrouter_request_body() {
        let provider = OpenRouterProvider::new(
            build_client(),
            "test-key".to_string(),
            "meta-llama/llama-3.1-8b-instruct:free".to_string(),
        );

        let body = provider.build_request_body("Be concise.", "Summarize this session.");

        assert_eq!(body["model"], "meta-llama/llama-3.1-8b-instruct:free");
        assert_eq!(body["max_tokens"], 1024);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "Be concise.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Summarize this session.");
    }

    // ==================== Response parsing tests ====================

    #[test]
    fn test_parse_openai_response_valid() {
        let json = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "This session implemented a new feature."
                    }
                }
            ]
        });

        let result = parse_openai_response(&json).unwrap();
        assert_eq!(result.content, "This session implemented a new feature.");
    }

    #[test]
    fn test_parse_openai_response_missing_choices() {
        let json = serde_json::json!({});
        let err = parse_openai_response(&json).unwrap_err();
        match err {
            SummarizeError::ParseError(msg) => {
                assert!(msg.contains("choices[0].message.content"));
            }
            other => panic!("Expected ParseError, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_openai_response_empty_choices() {
        let json = serde_json::json!({ "choices": [] });
        let err = parse_openai_response(&json).unwrap_err();
        match err {
            SummarizeError::ParseError(_) => {}
            other => panic!("Expected ParseError, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_openai_response_missing_content() {
        let json = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant"
                    }
                }
            ]
        });
        let err = parse_openai_response(&json).unwrap_err();
        match err {
            SummarizeError::ParseError(_) => {}
            other => panic!("Expected ParseError, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_anthropic_response_valid() {
        // Simulate what the Anthropic provider extracts.
        let json = serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": "This session refactored the database layer."
                }
            ]
        });

        let content = json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .unwrap();

        assert_eq!(content, "This session refactored the database layer.");
    }

    // ==================== Timeout constant tests ====================

    #[test]
    fn test_timeout_constants() {
        assert_eq!(CONNECT_TIMEOUT.as_secs(), 30);
        assert_eq!(REQUEST_TIMEOUT.as_secs(), 120);
    }

    // ==================== SummaryResponse tests ====================

    #[test]
    fn test_summary_response_debug() {
        let response = SummaryResponse {
            content: "test summary".to_string(),
        };
        let debug = format!("{response:?}");
        assert!(debug.contains("test summary"));
    }

    #[test]
    fn test_summary_response_clone() {
        let response = SummaryResponse {
            content: "test summary".to_string(),
        };
        let cloned = response.clone();
        assert_eq!(response.content, cloned.content);
    }
}

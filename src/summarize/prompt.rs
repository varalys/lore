//! Prompt construction for LLM-powered session summaries.
//!
//! Provides the system prompt and message formatting logic needed to
//! generate concise summaries of AI-assisted development sessions.

use crate::storage::models::{Message, MessageRole};

/// Returns the system prompt that instructs the LLM how to summarize a session.
///
/// The prompt directs the model to produce a one-sentence overview followed by
/// 2-5 bullet points covering the key technical work, staying under 300 words
/// total. Output is plain text with no markdown headers.
pub fn system_prompt() -> &'static str {
    "You are summarizing an AI-assisted coding session. \
     Produce a one-sentence overview of what the session accomplished, \
     followed by 2-5 bullet points covering the key technical work.\n\n\
     Rules:\n\
     - Keep the total summary under 300 words.\n\
     - Focus on what was done and why, not how.\n\
     - Do not mention tool calls or internal mechanics.\n\
     - Use plain text only. No markdown headers, no special formatting.\n\
     - Start bullet points with a dash (-)."
}

/// Formats session messages into a conversation transcript for the LLM.
///
/// Each message is rendered with a role tag and, for user messages, a UTC
/// timestamp. Tool calls and thinking blocks are excluded via
/// `MessageContent::text()`.
///
/// If the formatted output exceeds `max_chars`, the middle portion of the
/// conversation is replaced with an omission marker. The first 20% and
/// last 30% of messages are kept to preserve context from both the beginning
/// and end of the session.
///
/// Returns an empty string when the message slice is empty.
pub fn prepare_conversation(messages: &[Message], max_chars: usize) -> String {
    if messages.is_empty() {
        return String::new();
    }

    let formatted = format_messages(messages);

    if max_chars == 0 || formatted.len() <= max_chars {
        return formatted;
    }

    // Truncation: keep first 20% and last 30% of messages
    truncate_conversation(messages, max_chars)
}

/// Formats a slice of messages into the conversation transcript.
fn format_messages(messages: &[Message]) -> String {
    let mut parts = Vec::with_capacity(messages.len());

    for msg in messages {
        let text = msg.content.text();
        let formatted = format_single_message(msg, &text);
        if !formatted.is_empty() {
            parts.push(formatted);
        }
    }

    parts.join("\n\n")
}

/// Formats a single message with its role header and content.
///
/// User and system messages include a UTC timestamp. Assistant messages
/// show only the role tag. Returns an empty string if the message text
/// is empty (e.g., messages containing only tool blocks).
fn format_single_message(msg: &Message, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let header = match msg.role {
        MessageRole::User => {
            let ts = msg.timestamp.format("%Y-%m-%d %H:%M UTC");
            format!("[User] ({ts})")
        }
        MessageRole::Assistant => "[Assistant]".to_string(),
        MessageRole::System => {
            let ts = msg.timestamp.format("%Y-%m-%d %H:%M UTC");
            format!("[System] ({ts})")
        }
    };

    format!("{header}\n{text}")
}

/// Applies the head+tail truncation strategy when the full transcript is too long.
///
/// Keeps the first 20% of messages and the last 30%, replacing the middle
/// with an omission marker indicating how many messages were skipped.
fn truncate_conversation(messages: &[Message], max_chars: usize) -> String {
    let count = messages.len();

    // Calculate how many messages to keep from head and tail
    let head_count = ((count as f64) * 0.2).ceil() as usize;
    let tail_count = ((count as f64) * 0.3).ceil() as usize;

    // Ensure we don't overlap (when message count is very small)
    let (head_count, tail_count) = if head_count + tail_count >= count {
        // Not enough messages to truncate; just format all of them
        return format_messages(messages);
    } else {
        (head_count, tail_count)
    };

    let omitted = count - head_count - tail_count;
    let head_msgs = &messages[..head_count];
    let tail_msgs = &messages[count - tail_count..];

    let head_text = format_messages(head_msgs);
    let marker = format!("[... {omitted} messages omitted ...]");
    let tail_text = format_messages(tail_msgs);

    let result = format!("{head_text}\n\n{marker}\n\n{tail_text}");

    // If still over the limit after truncation, hard-truncate the result
    if result.len() > max_chars {
        let truncated: String = result.chars().take(max_chars).collect();
        truncated
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{ContentBlock, MessageContent};
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    /// Helper to create a test message with the given role and text content.
    fn make_message(role: MessageRole, text: &str, index: i32) -> Message {
        Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index,
            timestamp: Utc.with_ymd_and_hms(2024, 1, 15, 14, 30, 0).unwrap(),
            role,
            content: MessageContent::Text(text.to_string()),
            model: None,
            git_branch: None,
            cwd: None,
        }
    }

    /// Helper to create a message with only tool-use blocks (no text).
    fn make_tool_only_message(index: i32) -> Message {
        Message {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            parent_id: None,
            index,
            timestamp: Utc.with_ymd_and_hms(2024, 1, 15, 14, 30, 0).unwrap(),
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            }]),
            model: None,
            git_branch: None,
            cwd: None,
        }
    }

    #[test]
    fn test_system_prompt_is_non_empty() {
        let prompt = system_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("summariz"));
        assert!(prompt.contains("300 words"));
    }

    #[test]
    fn test_basic_user_and_assistant_formatting() {
        let messages = vec![
            make_message(MessageRole::User, "Fix the login bug", 0),
            make_message(MessageRole::Assistant, "I found the issue in auth.rs", 1),
        ];

        let result = prepare_conversation(&messages, 10_000);

        assert!(result.contains("[User] (2024-01-15 14:30 UTC)"));
        assert!(result.contains("Fix the login bug"));
        assert!(result.contains("[Assistant]"));
        assert!(result.contains("I found the issue in auth.rs"));
        // Assistant messages should not have a timestamp in the header
        assert!(!result.contains("[Assistant] ("));
    }

    #[test]
    fn test_system_message_formatting() {
        let messages = vec![make_message(
            MessageRole::System,
            "You are a coding assistant",
            0,
        )];

        let result = prepare_conversation(&messages, 10_000);

        assert!(result.contains("[System] (2024-01-15 14:30 UTC)"));
        assert!(result.contains("You are a coding assistant"));
    }

    #[test]
    fn test_empty_messages_returns_empty_string() {
        let messages: Vec<Message> = vec![];
        let result = prepare_conversation(&messages, 10_000);
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_message_formatting() {
        let messages = vec![make_message(MessageRole::User, "Hello", 0)];

        let result = prepare_conversation(&messages, 10_000);

        assert_eq!(result, "[User] (2024-01-15 14:30 UTC)\nHello");
    }

    #[test]
    fn test_tool_only_messages_handled_gracefully() {
        let messages = vec![
            make_message(MessageRole::User, "Read that file", 0),
            make_tool_only_message(1),
            make_message(MessageRole::Assistant, "Done reading", 2),
        ];

        let result = prepare_conversation(&messages, 10_000);

        // The tool-only message should be skipped (empty text)
        assert!(result.contains("Read that file"));
        assert!(result.contains("Done reading"));
        // Should not contain back-to-back blank sections from the skipped message
        assert!(!result.contains("\n\n\n\n"));
    }

    #[test]
    fn test_truncation_with_head_and_tail_strategy() {
        // Create 20 messages so truncation math is clear:
        // head = ceil(20 * 0.2) = 4, tail = ceil(20 * 0.3) = 6, omitted = 10
        let mut messages = Vec::new();
        for i in 0..20 {
            let role = if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            messages.push(make_message(role, &format!("Message number {i}"), i));
        }

        // Use a limit smaller than the full output (~800 chars) but large enough
        // to contain the truncated head+marker+tail (~450 chars).
        let result = prepare_conversation(&messages, 500);

        assert!(result.contains("Message number 0"));
        assert!(result.contains("[... 10 messages omitted ...]"));
        assert!(result.contains("Message number 19"));
    }

    #[test]
    fn test_no_truncation_when_under_limit() {
        let messages = vec![
            make_message(MessageRole::User, "Short", 0),
            make_message(MessageRole::Assistant, "Reply", 1),
        ];

        let result = prepare_conversation(&messages, 10_000);

        assert!(!result.contains("omitted"));
    }

    #[test]
    fn test_truncation_preserves_first_and_last_messages() {
        // 10 messages: head = ceil(10*0.2)=2, tail = ceil(10*0.3)=3, omitted = 5
        let mut messages = Vec::new();
        for i in 0..10 {
            messages.push(make_message(MessageRole::User, &format!("msg-{i}"), i));
        }

        // Full output is ~380 chars. Use 250 to trigger truncation but
        // leave enough room for the head+marker+tail (~220 chars).
        let result = prepare_conversation(&messages, 250);

        // Head messages
        assert!(result.contains("msg-0"));
        assert!(result.contains("msg-1"));
        // Tail messages
        assert!(result.contains("msg-7"));
        assert!(result.contains("msg-8"));
        assert!(result.contains("msg-9"));
        // Omission marker
        assert!(result.contains("[... 5 messages omitted ...]"));
    }

    #[test]
    fn test_all_tool_only_messages_produce_empty_output() {
        let messages = vec![make_tool_only_message(0), make_tool_only_message(1)];

        let result = prepare_conversation(&messages, 10_000);

        assert!(result.is_empty());
    }

    #[test]
    fn test_max_chars_zero_returns_full_conversation() {
        let messages = vec![
            make_message(MessageRole::User, "Hello", 0),
            make_message(MessageRole::Assistant, "World", 1),
        ];

        let result = prepare_conversation(&messages, 0);

        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn test_few_messages_no_truncation_when_head_tail_covers_all() {
        // With 2 messages: head = ceil(2*0.2)=1, tail = ceil(2*0.3)=1
        // head + tail = 2 >= count => no truncation possible, returns full text
        let two_messages = vec![
            make_message(MessageRole::User, "Alpha", 0),
            make_message(MessageRole::Assistant, "Beta", 1),
        ];

        // Even with a small limit, the truncation logic sees that
        // head+tail covers all messages and returns the full output.
        let full = prepare_conversation(&two_messages, 10_000);
        let truncated = prepare_conversation(&two_messages, 1);

        // The result is the full text (possibly hard-truncated), but
        // no omission marker because there are too few messages to split.
        assert!(!full.contains("omitted"));
        // Hard truncation may apply, but the omission marker is not present
        // in the underlying truncation output.
        assert!(truncated.len() <= full.len());
    }

    #[test]
    fn test_small_message_set_truncation() {
        // Use 5 messages with long enough content so omitting the middle
        // saves more space than the marker takes.
        // head = ceil(5*0.2)=1, tail = ceil(5*0.3)=2, omitted = 2
        let messages = vec![
            make_message(
                MessageRole::User,
                "The very first user message in the session",
                0,
            ),
            make_message(
                MessageRole::Assistant,
                "A long middle response that should be omitted from output",
                1,
            ),
            make_message(
                MessageRole::User,
                "Another middle message that should be omitted from output",
                2,
            ),
            make_message(
                MessageRole::Assistant,
                "The penultimate message in the tail section",
                3,
            ),
            make_message(
                MessageRole::User,
                "The final message in the conversation",
                4,
            ),
        ];

        let full = prepare_conversation(&messages, 10_000);
        // Truncated output replaces 2 messages (~140 chars) with a marker (~30 chars)
        // so it should be shorter. Use a limit that triggers truncation.
        let result = prepare_conversation(&messages, full.len() - 1);

        assert!(result.contains("The very first user message"));
        assert!(result.contains("penultimate message"));
        assert!(result.contains("final message"));
        assert!(result.contains("[... 2 messages omitted ...]"));
        // Omitted messages should not appear
        assert!(!result.contains("A long middle response"));
        assert!(!result.contains("Another middle message"));
    }
}

//! Blame command - show which AI session led to a specific line of code.
//!
//! Uses git blame to find the commit that introduced a line, then looks up
//! any sessions linked to that commit and displays relevant context from
//! those sessions.

use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::storage::{Database, Message, Session};

/// Arguments for the blame command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore blame src/main.rs:42         Show session for line 42\n    \
    lore blame src/lib.rs:10 -f json  Output as JSON\n    \
    lore blame Cargo.toml:1           Show session for first line")]
pub struct Args {
    /// File and line number in the format file:line (e.g., src/main.rs:42)
    #[arg(value_name = "FILE:LINE")]
    #[arg(long_help = "The file path and line number to look up, separated by\n\
        a colon. The file path is relative to the repository root.\n\
        Example: src/main.rs:42")]
    pub target: String,

    /// Output format: text (default), json, or markdown
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

/// JSON output structure for blame results.
#[derive(Serialize)]
struct BlameOutput {
    file: String,
    line: usize,
    commit_sha: String,
    commit_short: String,
    commit_summary: String,
    commit_author: String,
    commit_date: String,
    line_content: String,
    sessions: Vec<BlameSessionInfo>,
}

/// Session information in blame output.
#[derive(Serialize)]
struct BlameSessionInfo {
    session_id: String,
    tool: String,
    started_at: String,
    message_count: i32,
    relevant_excerpts: Vec<String>,
}

/// Executes the blame command.
///
/// Parses the file:line argument, runs git blame to find the commit,
/// then looks up any linked sessions and extracts relevant context.
pub fn run(args: Args) -> Result<()> {
    // Parse file:line argument
    let (file_path, line_num) = parse_file_line(&args.target)?;

    // Run git blame to find the commit
    let blame_info = git_blame(&file_path, line_num)?;

    // Open the database and find linked sessions
    let db = Database::open_default()?;
    let links = db.get_links_by_commit(&blame_info.commit_sha)?;

    // Gather session info with relevant excerpts
    let mut session_infos = Vec::new();
    for link in &links {
        if let Some(session) = db.get_session(&link.session_id)? {
            let messages = db.get_messages(&session.id)?;
            let excerpts = find_relevant_excerpts(&messages, &file_path, &blame_info.line_content);
            session_infos.push((session, excerpts));
        }
    }

    // Output results
    match args.format {
        OutputFormat::Json => {
            print_json(&file_path, line_num, &blame_info, &session_infos)?;
        }
        OutputFormat::Markdown => {
            print_markdown(&file_path, line_num, &blame_info, &session_infos);
        }
        OutputFormat::Text => {
            print_text(&file_path, line_num, &blame_info, &session_infos);
        }
    }

    Ok(())
}

/// Information extracted from git blame for a specific line.
struct BlameInfo {
    commit_sha: String,
    commit_short: String,
    author: String,
    date: String,
    summary: String,
    line_content: String,
}

/// Parses a file:line argument into its components.
///
/// Returns an error if the format is invalid or the line number is not positive.
fn parse_file_line(target: &str) -> Result<(String, usize)> {
    // Find the last colon (to handle Windows paths like C:\path\file.rs:10)
    let last_colon = target
        .rfind(':')
        .context("Invalid format. Expected file:line (e.g., src/main.rs:42)")?;

    let file_path = &target[..last_colon];
    let line_str = &target[last_colon + 1..];

    if file_path.is_empty() {
        anyhow::bail!("File path cannot be empty. Expected file:line (e.g., src/main.rs:42)");
    }

    let line_num: usize = line_str
        .parse()
        .context("Invalid line number. Expected a positive integer.")?;

    if line_num == 0 {
        anyhow::bail!("Line number must be positive (1-indexed).");
    }

    Ok((file_path.to_string(), line_num))
}

/// Runs git blame on a specific line and extracts commit information.
fn git_blame(file_path: &str, line_num: usize) -> Result<BlameInfo> {
    let repo = git2::Repository::discover(".")
        .context("Not in a git repository. Run this command from within a git repository.")?;

    let workdir = repo
        .workdir()
        .context("Could not determine repository working directory")?;

    // Resolve the file path relative to the repo root
    let abs_path = if Path::new(file_path).is_absolute() {
        file_path.to_string()
    } else {
        workdir.join(file_path).to_string_lossy().to_string()
    };

    // Check if file exists
    if !Path::new(&abs_path).exists() {
        anyhow::bail!("File not found: {file_path}");
    }

    // Read the file to get line content and validate line number
    let content = std::fs::read_to_string(&abs_path)
        .with_context(|| format!("Could not read file: {file_path}"))?;

    let lines: Vec<&str> = content.lines().collect();
    if line_num > lines.len() {
        anyhow::bail!(
            "Line {line_num} is out of range. File has {} lines.",
            lines.len()
        );
    }

    let line_content = lines[line_num - 1].to_string();

    // Get the relative path for git operations
    let rel_path = if Path::new(file_path).is_absolute() {
        Path::new(file_path)
            .strip_prefix(workdir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_path.to_string())
    } else {
        file_path.to_string()
    };

    // Run git blame
    let blame = repo
        .blame_file(Path::new(&rel_path), None)
        .with_context(|| format!("Could not run git blame on: {file_path}"))?;

    // Get the hunk for our line (git blame uses 0-indexed lines internally)
    let hunk = blame
        .get_line(line_num)
        .with_context(|| format!("Could not get blame info for line {line_num}"))?;

    let commit_id = hunk.final_commit_id();
    let commit_sha = commit_id.to_string();
    let commit_short = commit_sha[..8.min(commit_sha.len())].to_string();

    // Get more commit details
    let commit = repo
        .find_commit(commit_id)
        .with_context(|| format!("Could not find commit: {commit_sha}"))?;

    let author = commit.author().name().unwrap_or("Unknown").to_string();
    let summary = commit.summary().unwrap_or("").to_string();

    // Format the commit date
    let time = commit.time();
    let date = chrono::DateTime::from_timestamp(time.seconds(), 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    Ok(BlameInfo {
        commit_sha,
        commit_short,
        author,
        date,
        summary,
        line_content,
    })
}

/// Finds relevant excerpts from messages that mention the file or contain similar code.
///
/// Searches for:
/// 1. Messages that mention the file path
/// 2. Messages that contain code similar to the blamed line
fn find_relevant_excerpts(
    messages: &[Message],
    file_path: &str,
    line_content: &str,
) -> Vec<String> {
    let mut excerpts = Vec::new();
    let file_name = Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file_path);

    // Normalize the line content for comparison (trim whitespace)
    let normalized_line = line_content.trim();

    for msg in messages {
        let text = msg.content.text();
        if text.is_empty() {
            continue;
        }

        // Check if the message mentions the file
        let mentions_file = text.contains(file_path) || text.contains(file_name);

        // Check if the message contains similar code
        let contains_code = !normalized_line.is_empty() && text.contains(normalized_line);

        if mentions_file || contains_code {
            // Extract a relevant snippet (up to 200 chars around the match)
            let snippet = extract_snippet(&text, file_path, file_name, normalized_line);
            if !snippet.is_empty() && !excerpts.contains(&snippet) {
                excerpts.push(snippet);
            }
        }
    }

    // Limit to 5 excerpts
    excerpts.truncate(5);
    excerpts
}

/// Extracts a snippet from text around a match.
fn extract_snippet(text: &str, file_path: &str, file_name: &str, line_content: &str) -> String {
    // Find the position of the match
    let pos = text
        .find(file_path)
        .or_else(|| text.find(file_name))
        .or_else(|| {
            if !line_content.is_empty() {
                text.find(line_content)
            } else {
                None
            }
        });

    if let Some(pos) = pos {
        // Extract ~100 chars before and after
        let start = pos.saturating_sub(100);
        let end = (pos + 100).min(text.len());

        // Find word boundaries
        let start = text[..start]
            .rfind(char::is_whitespace)
            .map(|p| p + 1)
            .unwrap_or(start);
        let end = text[end..]
            .find(char::is_whitespace)
            .map(|p| end + p)
            .unwrap_or(end);

        let mut snippet = text[start..end].to_string();

        // Add ellipsis if truncated
        if start > 0 {
            snippet = format!("...{snippet}");
        }
        if end < text.len() {
            snippet = format!("{snippet}...");
        }

        // Clean up whitespace
        snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");

        return snippet;
    }

    String::new()
}

/// Prints blame results in JSON format.
fn print_json(
    file_path: &str,
    line_num: usize,
    blame_info: &BlameInfo,
    session_infos: &[(Session, Vec<String>)],
) -> Result<()> {
    let output = BlameOutput {
        file: file_path.to_string(),
        line: line_num,
        commit_sha: blame_info.commit_sha.clone(),
        commit_short: blame_info.commit_short.clone(),
        commit_summary: blame_info.summary.clone(),
        commit_author: blame_info.author.clone(),
        commit_date: blame_info.date.clone(),
        line_content: blame_info.line_content.clone(),
        sessions: session_infos
            .iter()
            .map(|(s, excerpts)| BlameSessionInfo {
                session_id: s.id.to_string(),
                tool: s.tool.clone(),
                started_at: s.started_at.to_rfc3339(),
                message_count: s.message_count,
                relevant_excerpts: excerpts.clone(),
            })
            .collect(),
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{json}");
    Ok(())
}

/// Prints blame results in markdown format.
fn print_markdown(
    file_path: &str,
    line_num: usize,
    blame_info: &BlameInfo,
    session_infos: &[(Session, Vec<String>)],
) {
    println!("# Blame: `{file_path}:{line_num}`");
    println!();

    println!("## Commit");
    println!();
    println!("| Property | Value |");
    println!("|----------|-------|");
    println!("| SHA | `{}` |", blame_info.commit_short);
    println!("| Author | {} |", blame_info.author);
    println!("| Date | {} |", blame_info.date);
    println!("| Summary | {} |", blame_info.summary);
    println!();

    println!("## Line Content");
    println!();
    println!("```");
    println!("{}", blame_info.line_content);
    println!("```");
    println!();

    if session_infos.is_empty() {
        println!("*No linked sessions found for this commit.*");
    } else {
        println!("## Linked Sessions ({})", session_infos.len());
        println!();

        for (session, excerpts) in session_infos {
            let id_short = &session.id.to_string()[..8];
            println!("### Session `{id_short}`");
            println!();
            println!("- **Tool:** {}", session.tool);
            println!(
                "- **Started:** {}",
                session.started_at.format("%Y-%m-%d %H:%M")
            );
            println!("- **Messages:** {}", session.message_count);
            println!();

            if !excerpts.is_empty() {
                println!("**Relevant excerpts:**");
                println!();
                for excerpt in excerpts {
                    println!("> {excerpt}");
                    println!();
                }
            }
        }
    }
}

/// Prints blame results in text format with colors.
fn print_text(
    file_path: &str,
    line_num: usize,
    blame_info: &BlameInfo,
    session_infos: &[(Session, Vec<String>)],
) {
    println!(
        "{} {}:{}",
        "Blame".bold(),
        file_path.cyan(),
        line_num.to_string().yellow()
    );
    println!();

    println!("{}", "Commit:".bold());
    println!(
        "  {}  {} {}",
        blame_info.commit_short.yellow(),
        blame_info.author.dimmed(),
        blame_info.date.dimmed()
    );
    println!("  {}", blame_info.summary);
    println!();

    println!("{}", "Line content:".bold());
    println!("  {}", blame_info.line_content.dimmed());
    println!();

    if session_infos.is_empty() {
        println!("{}", "No linked sessions found for this commit.".dimmed());
        println!();
        println!(
            "{}",
            "Use 'lore link <session> --commit <sha>' to link a session.".dimmed()
        );
    } else {
        println!(
            "{}",
            format!("Linked sessions ({}):", session_infos.len()).bold()
        );

        for (session, excerpts) in session_infos {
            let id_short = &session.id.to_string()[..8];
            println!();
            println!(
                "  {}  {} ({} messages)",
                id_short.cyan(),
                session.tool.dimmed(),
                session.message_count
            );
            println!(
                "    {} {}",
                "Started:".dimmed(),
                session.started_at.format("%Y-%m-%d %H:%M")
            );

            if !excerpts.is_empty() {
                println!("    {}", "Relevant context:".dimmed());
                for excerpt in excerpts.iter().take(3) {
                    // Truncate long excerpts for terminal display
                    let display = if excerpt.len() > 100 {
                        format!("{}...", &excerpt[..100])
                    } else {
                        excerpt.clone()
                    };
                    println!("      {}", display.dimmed());
                }
            }
        }

        println!();
        println!(
            "{}",
            "Use 'lore show <session-id>' to view full session details.".dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MessageContent;

    #[test]
    fn test_parse_file_line_valid() {
        let (file, line) = parse_file_line("src/main.rs:42").unwrap();
        assert_eq!(file, "src/main.rs");
        assert_eq!(line, 42);
    }

    #[test]
    fn test_parse_file_line_line_one() {
        let (file, line) = parse_file_line("Cargo.toml:1").unwrap();
        assert_eq!(file, "Cargo.toml");
        assert_eq!(line, 1);
    }

    #[test]
    fn test_parse_file_line_nested_path() {
        let (file, line) = parse_file_line("src/cli/commands/blame.rs:100").unwrap();
        assert_eq!(file, "src/cli/commands/blame.rs");
        assert_eq!(line, 100);
    }

    #[test]
    fn test_parse_file_line_missing_colon() {
        let result = parse_file_line("src/main.rs");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_file_line_invalid_line_number() {
        let result = parse_file_line("src/main.rs:abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_file_line_zero_line() {
        let result = parse_file_line("src/main.rs:0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_file_line_empty_file() {
        let result = parse_file_line(":42");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_snippet_file_match() {
        let text = "I'm going to read the src/main.rs file to understand the structure.";
        let snippet = extract_snippet(text, "src/main.rs", "main.rs", "");
        assert!(snippet.contains("src/main.rs"));
    }

    #[test]
    fn test_extract_snippet_code_match() {
        let text = "Here's the code:\nfn main() {\n    println!(\"Hello\");\n}";
        let snippet = extract_snippet(text, "nonexistent.rs", "nonexistent.rs", "fn main()");
        assert!(snippet.contains("fn main()"));
    }

    #[test]
    fn test_extract_snippet_no_match() {
        let text = "This text has nothing relevant.";
        let snippet = extract_snippet(text, "other.rs", "other.rs", "unrelated code");
        assert!(snippet.is_empty());
    }

    #[test]
    fn test_find_relevant_excerpts_empty_messages() {
        let messages: Vec<Message> = vec![];
        let excerpts = find_relevant_excerpts(&messages, "src/main.rs", "fn main()");
        assert!(excerpts.is_empty());
    }

    #[test]
    fn test_find_relevant_excerpts_limits_to_five() {
        use chrono::Utc;
        use uuid::Uuid;

        let messages: Vec<Message> = (0..10)
            .map(|i| Message {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                parent_id: None,
                index: i,
                timestamp: Utc::now(),
                role: crate::storage::MessageRole::User,
                content: MessageContent::Text(format!(
                    "Message {i} mentions src/main.rs and some content"
                )),
                model: None,
                git_branch: None,
                cwd: None,
            })
            .collect();

        let excerpts = find_relevant_excerpts(&messages, "src/main.rs", "content");
        assert!(excerpts.len() <= 5);
    }
}

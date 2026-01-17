//! Login command - authenticate with Lore cloud service.
//!
//! Opens a browser for OAuth authentication and stores the resulting
//! API key in the OS keychain or a fallback credentials file.

use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::cloud::credentials::{Credentials, CredentialsStore};
use crate::cloud::DEFAULT_CLOUD_URL;
use crate::config::Config;

/// Arguments for the login command.
#[derive(clap::Args)]
#[command(after_help = "EXAMPLES:\n    \
    lore login                 Authenticate with Lore cloud")]
pub struct Args {
    /// Cloud service URL (for self-hosted deployments).
    #[arg(long)]
    pub url: Option<String>,
}

/// Timeout for waiting for browser callback.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(120);

/// Executes the login command.
///
/// Opens a browser to the cloud service OAuth page, waits for the callback
/// with credentials, and stores them securely.
pub fn run(args: Args) -> Result<()> {
    // Check if already logged in
    let store = CredentialsStore::new();
    if let Ok(Some(creds)) = store.load() {
        println!(
            "Already logged in as {} ({} plan)",
            creds.email.cyan(),
            creds.plan
        );
        println!("Run 'lore logout' first to log out.");
        return Ok(());
    }

    // Determine cloud URL
    let config = Config::load()?;
    let cloud_url = args
        .url
        .as_deref()
        .unwrap_or_else(|| config.cloud_url.as_deref().unwrap_or(DEFAULT_CLOUD_URL));

    // Start local HTTP server on a random available port
    let listener =
        TcpListener::bind("127.0.0.1:0").context("Failed to start local callback server")?;
    let port = listener.local_addr()?.port();

    // Generate random state parameter for CSRF protection
    let state = generate_state();

    // Build OAuth URL
    let auth_url = format!(
        "{}/auth/cli?port={}&state={}",
        cloud_url.trim_end_matches('/'),
        port,
        state
    );

    println!("Opening browser for authentication...");
    println!();
    println!("If the browser does not open, visit:");
    println!("  {}", auth_url.cyan());
    println!();

    // Open browser
    if let Err(e) = webbrowser::open(&auth_url) {
        eprintln!("Failed to open browser: {e}");
        println!("Please open the URL above manually.");
    }

    // Wait for callback with timeout
    listener
        .set_nonblocking(true)
        .context("Failed to set non-blocking mode")?;

    let start = Instant::now();
    let credentials = loop {
        if start.elapsed() > LOGIN_TIMEOUT {
            bail!("Login timed out waiting for browser authentication");
        }

        match listener.accept() {
            Ok((stream, _)) => {
                match handle_callback(stream, &state) {
                    Ok(creds) => break creds,
                    Err(e) => {
                        // Log error but continue waiting - might be browser prefetch
                        tracing::debug!("Callback error (will retry): {e}");
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No connection yet, wait a bit
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                bail!("Failed to accept connection: {e}");
            }
        }
    };

    // Store credentials
    store
        .store(&credentials)
        .context("Failed to store credentials")?;

    println!();
    println!(
        "{} Logged in as {} ({} plan)",
        "Success!".green().bold(),
        credentials.email.cyan(),
        credentials.plan
    );

    Ok(())
}

/// Generates a random state string for CSRF protection.
fn generate_state() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Handles the OAuth callback request.
///
/// Parses the callback URL parameters, validates the state, and extracts credentials.
fn handle_callback(mut stream: TcpStream, expected_state: &str) -> Result<Credentials> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("Failed to set read timeout")?;

    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("Failed to read request")?;

    // Parse the request line: GET /callback?key=...&state=...&email=...&plan=... HTTP/1.1
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" {
        bail!("Invalid HTTP request");
    }

    let path = parts[1];
    if !path.starts_with("/callback?") {
        // Send 404 and continue waiting
        send_response(&mut stream, 404, "Not Found", "Invalid callback path");
        bail!("Invalid callback path");
    }

    // Parse query parameters
    let query = path.strip_prefix("/callback?").unwrap_or("");
    let params = parse_query_string(query);

    // Validate state
    let state = params
        .get("state")
        .ok_or_else(|| anyhow::anyhow!("Missing state parameter"))?;
    if state != expected_state {
        send_response(
            &mut stream,
            403,
            "Forbidden",
            "State mismatch - possible CSRF attack",
        );
        bail!("OAuth state mismatch - possible CSRF attack");
    }

    // Extract credentials
    let api_key = params
        .get("key")
        .ok_or_else(|| anyhow::anyhow!("Missing API key in callback"))?
        .to_string();

    let email = params
        .get("email")
        .ok_or_else(|| anyhow::anyhow!("Missing email in callback"))?
        .to_string();

    let plan = params
        .get("plan")
        .ok_or_else(|| anyhow::anyhow!("Missing plan in callback"))?
        .to_string();

    let cloud_url = params
        .get("url")
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT_CLOUD_URL.to_string());

    // Send success response to browser
    let success_html = r#"<!DOCTYPE html>
<html>
<head>
    <title>Lore - Login Successful</title>
    <style>
        body { font-family: system-ui; max-width: 500px; margin: 100px auto; text-align: center; }
        .success { color: #22c55e; font-size: 48px; }
        h1 { color: #333; }
        p { color: #666; }
    </style>
</head>
<body>
    <div class="success">&#10003;</div>
    <h1>Login Successful!</h1>
    <p>You can close this window and return to your terminal.</p>
</body>
</html>"#;

    send_response(&mut stream, 200, "OK", success_html);

    Ok(Credentials {
        api_key,
        email,
        plan,
        cloud_url,
    })
}

/// Parses a query string into key-value pairs.
fn parse_query_string(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            if pair.is_empty() {
                return None;
            }
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?;
            if key.is_empty() {
                return None;
            }
            let value = parts.next().unwrap_or("");
            Some((urlencoding_decode(key), urlencoding_decode(value)))
        })
        .collect()
}

/// Simple URL decoding (handles %XX escapes).
fn urlencoding_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

/// Sends an HTTP response.
fn send_response(stream: &mut TcpStream, status: u16, status_text: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {} {}\r\n\
        Content-Type: text/html\r\n\
        Content-Length: {}\r\n\
        Connection: close\r\n\
        \r\n\
        {}",
        status,
        status_text,
        body.len(),
        body
    );

    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_state_length() {
        let state = generate_state();
        assert_eq!(state.len(), 32); // 16 bytes = 32 hex chars
    }

    #[test]
    fn test_generate_state_uniqueness() {
        let state1 = generate_state();
        let state2 = generate_state();
        assert_ne!(state1, state2);
    }

    #[test]
    fn test_parse_query_string() {
        let params = parse_query_string("key=abc123&email=test@example.com&plan=pro&state=xyz");
        assert_eq!(params.get("key"), Some(&"abc123".to_string()));
        assert_eq!(params.get("email"), Some(&"test@example.com".to_string()));
        assert_eq!(params.get("plan"), Some(&"pro".to_string()));
        assert_eq!(params.get("state"), Some(&"xyz".to_string()));
    }

    #[test]
    fn test_parse_query_string_empty() {
        let params = parse_query_string("");
        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_query_string_encoded() {
        let params = parse_query_string("email=test%40example.com&name=John+Doe");
        assert_eq!(params.get("email"), Some(&"test@example.com".to_string()));
        assert_eq!(params.get("name"), Some(&"John Doe".to_string()));
    }

    #[test]
    fn test_urlencoding_decode() {
        assert_eq!(urlencoding_decode("hello%20world"), "hello world");
        assert_eq!(urlencoding_decode("test%40example.com"), "test@example.com");
        assert_eq!(urlencoding_decode("hello+world"), "hello world");
        assert_eq!(urlencoding_decode("no%encoding"), "no%encoding"); // Invalid escape
    }
}

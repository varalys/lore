# Lore

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![CI](https://github.com/redactyl/lore/actions/workflows/ci.yml/badge.svg)](https://github.com/redactyl/lore/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/redactyl/lore)](https://github.com/redactyl/lore/releases)

Lore captures AI coding sessions and links them to git commits.

When you use AI coding tools like Claude Code or Aider, the conversation history contains valuable context: the prompts you wrote, the approaches you tried, the decisions you made. Git captures the final code, but not this reasoning. Lore preserves both.

## Use Cases

- **Code review**: See the AI conversation that produced a PR, not just the diff
- **Debugging**: Understand why code was written a certain way by reading the original discussion
- **Knowledge transfer**: When someone leaves a project, their AI conversations stay with the code
- **Learning**: Study how problems were solved by browsing linked sessions

## How It Works

Lore reads session data from AI coding tools, stores it in a local SQLite database, and creates links between sessions and git commits.

### Capture

Lore includes parsers for each supported tool:

| Tool | Format |
|------|--------|
| Claude Code | JSONL |
| Codex CLI | JSONL |
| Gemini CLI | JSON |
| Amp | JSON |
| Aider | Markdown |
| Continue.dev | JSON |
| Cline | JSON |
| Roo Code | JSON |
| OpenCode | JSON |

Import existing sessions with `lore import`, or run `lore daemon start` to watch for new sessions in real-time.

### Storage

Sessions and messages are stored in a SQLite database at `~/.lore/lore.db`. The schema includes:

- **sessions**: ID, tool, timestamps, working directory, message count
- **messages**: ID, session ID, role (user/assistant), content, timestamp
- **session_links**: Maps session IDs to git commit SHAs

Full-text search uses SQLite FTS5 to index message content.

### Linking

Links connect sessions to commits. You can create them:

- **Manually**: `lore link <session-id> --commit <sha>`
- **Automatically**: `lore link --auto` matches sessions to commits by timestamp and file overlap
- **Via hooks**: `lore hooks install` adds a post-commit hook that prompts for linking

Links are bidirectional: given a session, find its commits; given a commit, find its sessions.

## Installation

### From Releases

Download the latest binary from [GitHub Releases](https://github.com/redactyl/lore/releases) and add it to your PATH.

### From Source

```bash
git clone https://github.com/redactyl/lore.git
cd lore
cargo install --path .
```

## Quick Start

```bash
# Import existing sessions from AI coding tools
lore import

# List sessions
lore sessions

# View a session
lore show abc123

# Link a session to the current commit
lore link abc123

# Later, view what sessions informed a commit
lore show --commit HEAD
```

## Example Workflow

```bash
# You're reviewing a PR and want to understand a change
$ git log --oneline -1
a1b2c3d feat: add rate limiting to API

$ lore show --commit a1b2c3d
Sessions linked to commit a1b2c3d:

  Session: 7f3a2b1
  Tool: claude-code
  Duration: 45 minutes
  Messages: 23

# View the full conversation
$ lore show 7f3a2b1
```

## Commands

| Command | Description |
|---------|-------------|
| `lore status` | Show daemon status, watchers, and recent sessions |
| `lore sessions` | List sessions (supports `--repo`, `--limit`, `--format`) |
| `lore show <id>` | View session details |
| `lore show --commit <ref>` | View sessions linked to a commit |
| `lore import` | Import sessions from AI tools |
| `lore link <id>` | Link session to HEAD |
| `lore link --auto` | Auto-link sessions by time and file overlap |
| `lore unlink <id>` | Remove a session-commit link |
| `lore search <query>` | Full-text search across all sessions |
| `lore hooks install` | Install git hooks for automatic linking |
| `lore daemon start` | Start background watcher for real-time capture |
| `lore daemon install` | Install daemon as a system service |
| `lore daemon uninstall` | Remove daemon service |
| `lore config` | View and update configuration |

## Supported Tools

| Tool | Status | Storage Location |
|------|--------|------------------|
| Claude Code | Supported | `~/.claude/projects/` |
| Codex CLI | Supported | `~/.codex/sessions/` |
| Gemini CLI | Supported | `~/.gemini/tmp/*/chats/` |
| Amp | Supported | `~/.local/share/amp/threads/` |
| Aider | Supported | `.aider.chat.history.md` |
| Continue.dev | Supported | `~/.continue/sessions/` |
| Cline | Supported | VS Code extension storage |
| Roo Code | Supported | VS Code extension storage |
| OpenCode | Supported | `~/.local/share/opencode/storage/` |

**Building an AI coding tool?** We welcome contributions to support additional tools. Open an issue with your tool's session storage location and format, or submit a PR adding a watcher. See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## Background Daemon

The daemon watches for new sessions in real-time and imports them automatically.

### Manual Start

```bash
lore daemon start    # Start watching
lore daemon status   # Check what's being watched
lore daemon logs     # View daemon logs
lore daemon stop     # Stop watching
```

### Run as a Service

Install the daemon as a system service to start automatically on login:

```bash
lore daemon install    # Install and enable service
lore daemon uninstall  # Remove service
```

This uses launchd on macOS and systemd on Linux. The service restarts automatically on failure.

#### Manual systemd Setup (Linux)

If you prefer to configure systemd yourself:

```bash
mkdir -p ~/.config/systemd/user
```

Create `~/.config/systemd/user/lore.service`:

```ini
[Unit]
Description=Lore AI session capture daemon
After=default.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/lore daemon start --foreground
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Then enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now lore.service
systemctl --user status lore.service
```

#### macOS with Homebrew

Once a Homebrew formula is available:

```bash
brew services start lore
brew services stop lore
```

Until then, use `lore daemon install` or manage launchd manually.

## Auto-linking

Lore can automatically link sessions to commits based on timing and file overlap:

```bash
# Preview what would be linked
lore link --auto --dry-run

# Link with default confidence threshold (0.5)
lore link --auto

# Require higher confidence
lore link --auto --threshold 0.7
```

## Git Hooks

Install hooks to automatically record session links on commit:

```bash
lore hooks install   # Install post-commit hook
lore hooks status    # Check hook status
lore hooks uninstall # Remove hooks
```

## Output Formats

Commands support `--format` for scripting and integration:

```bash
lore sessions --format json
lore show abc123 --format json
lore show abc123 --format markdown
lore status --format json
```

## Data Location

```
~/.lore/
├── lore.db       # SQLite database
├── config.yaml   # Configuration
└── logs/         # Daemon logs
```

All data stays on your machine. There is no cloud sync or external service.

## License

Apache 2.0

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

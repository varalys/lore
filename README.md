# Lore

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![CI](https://github.com/varalys/lore/actions/workflows/ci.yml/badge.svg)](https://github.com/varalys/lore/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/varalys/lore)](https://github.com/varalys/lore/releases)
![Platform](https://img.shields.io/badge/platform-linux%20%7C%20macOS%20%7C%20WSL2-blue)
![Windows](https://img.shields.io/badge/windows-planned-lightgrey)

Lore captures AI coding sessions and links them to git commits.

When you use AI coding tools like Claude Code or Aider, the conversation history contains valuable context. This includes everything from the prompts you wrote, the approaches you tried, and the decisions you made. Git captures the final code, but does not contain reasoning history for your commits. Lore preserves both.

## Table of Contents

- [Use Cases](#use-cases)
- [How It Works](#how-it-works)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Example Workflow](#example-workflow)
- [Search](#search)
- [Commands](#commands)
- [Supported Tools](#supported-tools)
- [Background Daemon](#background-daemon)
- [Git Hooks](#git-hooks)
- [Output Formats](#output-formats)
- [Configuration](#configuration)
- [Database Management](#database-management)
- [Session Deletion](#session-deletion)
- [Shell Completions](#shell-completions)
- [Data Location](#data-location)
- [License](#license)
- [Contributing](#contributing)

## Use Cases

- **Code review**: See the AI conversation that produced a PR, not just the diff
- **Debugging**: Understand why code was written a certain way by reading the original discussion
- **Knowledge transfer**: When someone leaves a project, their AI conversations stay with the code
- **Learning**: Study how problems were solved by browsing linked sessions
- **Search**: Find that conversation where you solved a similar problem - search by keyword, project, tool, or date

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
| Kilo Code | JSON |
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
- **Via hooks**: `lore hooks install` adds a post-commit hook that prompts for linking

Links are bidirectional: given a session, find its commits; given a commit, find its sessions.

## Installation

### Homebrew (macOS)

```bash
brew install varalys/tap/lore
```

### From crates.io

```bash
cargo install lore-cli
```

### From Releases

Download the latest binary from [GitHub Releases](https://github.com/varalys/lore/releases) and add it to your PATH.

### From Source

```bash
git clone https://github.com/varalys/lore.git
cd lore
cargo install --path .
```

## Quick Start

```bash
# First time? Run init for guided setup
lore init

# Or just start using lore - it will prompt for setup automatically
lore sessions

# Example output - shows branch transitions during each session
# ID        STARTED           MESSAGES  BRANCH                    DIRECTORY
# c9731a91  2025-12-25 17:52       566  main -> feat/auth -> main myapp
# 24af9690  2025-12-22 19:13      1910  feat/phase-0-foundati...  lore

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

## Search

Find any conversation across all your AI coding sessions:

```bash
# Basic search
lore search "authentication"

# Filter by tool
lore search "bug fix" --tool claude-code

# Filter by date range
lore search "refactor" --since 2025-12-01 --until 2025-12-15

# Filter by project or branch
lore search "api" --project myapp
lore search "feature" --branch main

# Combine filters
lore search "database" --tool aider --project backend --since 2025-12-01

# Show more context around matches
lore search "error handling" --context 3
```

Search matches message content, project names, branches, and tool names. Results show surrounding context so you can understand the conversation flow.

## Commands

| Command | Description |
|---------|-------------|
| `lore init` | Guided first-run setup (auto-detects AI tools) |
| `lore status` | Show daemon status, watchers, and recent sessions |
| `lore sessions` | List sessions with branch history (supports `--repo`, `--limit`, `--format`) |
| `lore show <id>` | View session details |
| `lore show --commit <ref>` | View sessions linked to a commit |
| `lore import` | Import sessions from all enabled watchers |
| `lore link <id>` | Link session to HEAD |
| `lore unlink <id>` | Remove a session-commit link |
| `lore delete <id>` | Permanently delete a session |
| `lore search <query>` | Full-text search with filters and context |
| `lore hooks install` | Install git hooks for automatic linking |
| `lore hooks status` | Check installed git hooks |
| `lore hooks uninstall` | Remove installed git hooks |
| `lore daemon start` | Start background watcher for real-time capture |
| `lore daemon stop` | Stop background watcher |
| `lore daemon logs` | View daemon logs |
| `lore daemon install` | Install daemon as a system service |
| `lore daemon uninstall` | Remove daemon service |
| `lore db stats` | Show database statistics |
| `lore db vacuum` | Reclaim unused disk space |
| `lore db prune` | Delete old sessions |
| `lore config` | View configuration |
| `lore config get <key>` | Get a config value |
| `lore config set <key> <val>` | Set a config value |
| `lore completions <shell>` | Generate shell completions |

## Supported Tools

Lore targets Linux, macOS, and WSL2. Windows native support is planned for a
future release. For WSL2, CLI-based tools work as long as the sessions live in
the Linux filesystem. VS Code extension sessions are only discovered when the
extensions run in WSL (Remote - WSL); if you run VS Code natively on Windows,
those sessions live under `%APPDATA%` and are not detected today.

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
| Kilo Code | Supported | VS Code extension storage |
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

```bash
brew services start lore
brew services stop lore
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

## Configuration

On first run, Lore prompts for setup automatically. You can also run `lore init` manually.

The init wizard:
1. Detects installed AI coding tools
2. Shows which tools have existing sessions
3. Lets you choose which watchers to enable
4. Offers to import existing sessions
5. Offers to install shell completions
6. Offers to start the background service (for real-time capture)

Configure which tools to track:

```bash
lore config set watchers claude-code,aider,gemini
lore config get watchers
```

For scripting, use `--no-init` to skip the first-run prompt:

```bash
lore --no-init sessions --format json
```

## Database Management

Lore provides commands for managing the database:

```bash
# View database statistics
lore db stats

# Example output:
# Database Statistics
#
#   Sessions:  142
#   Messages:  8934
#   Links:     67
#   File size: 12.45 MB
#
# Date Range
#   Oldest:   2024-06-15 09:23
#   Newest:   2025-01-02 14:56
#
# Sessions by Tool
#    claude-code:  98
#          aider:  31
#         gemini:  13

# Reclaim unused disk space
lore db vacuum

# Delete old sessions (preview first with --dry-run)
lore db prune --older-than 90d --dry-run
lore db prune --older-than 6m --force
```

Duration formats for `--older-than`:
- `Nd` - days (e.g., `90d`)
- `Nw` - weeks (e.g., `12w`)
- `Nm` - months (e.g., `6m`)
- `Ny` - years (e.g., `1y`)

## Session Deletion

Delete a single session and all its data:

```bash
lore delete abc123
```

This permanently removes the session, its messages, and any commit links.

## Shell Completions

The easiest way to install completions is to let Lore auto-detect your shell:

```bash
lore completions install
```

Or specify a shell explicitly:

```bash
lore completions install --shell fish
```

You can also output completions to stdout for manual installation:

```bash
lore completions bash > ~/.local/share/bash-completion/completions/lore
lore completions zsh > ~/.zfunc/_lore
lore completions fish > ~/.config/fish/completions/lore.fish
```

After installing, restart your shell or source the completion file.

PowerShell and Elvish completions are also available (`lore completions powershell`, `lore completions elvish`) and will be documented when Windows support is added.

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

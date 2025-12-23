# Lore

Lore captures AI coding sessions and links them to git commits.

Git tracks what changed. Lore tracks the reasoning behind those changes.

## Installation

```bash
cargo install --path .
```

## Quick Start

```bash
# Import sessions from AI coding tools
lore import

# List sessions
lore sessions

# View a session
lore show abc123

# Link a session to HEAD
lore link abc123

# View sessions linked to a commit
lore show --commit HEAD

# Search sessions
lore search "authentication"

# Start background daemon
lore daemon start
```

## Commands

| Command | Description |
|---------|-------------|
| `lore status` | Show daemon status, watchers, and recent sessions |
| `lore sessions` | List sessions |
| `lore show <id>` | View session details |
| `lore show --commit <ref>` | View sessions linked to a commit |
| `lore import` | Import sessions from AI tools |
| `lore link <id>` | Link session to HEAD |
| `lore link --auto` | Auto-link sessions by time and file overlap |
| `lore unlink <id>` | Remove a session-commit link |
| `lore search <query>` | Full-text search |
| `lore hooks install` | Install git hooks |
| `lore hooks uninstall` | Remove git hooks |
| `lore daemon start` | Start background watcher |
| `lore daemon stop` | Stop daemon |
| `lore daemon status` | Check daemon status |
| `lore daemon logs` | View daemon logs |
| `lore config` | View configuration |
| `lore config set <key> <value>` | Update configuration |

## Supported Tools

| Tool | Status | Storage Location |
|------|--------|------------------|
| Claude Code | Supported | `~/.claude/projects/` |
| Aider | Supported | `.aider.chat.history.md` |
| Continue.dev | Supported | `~/.continue/sessions/` |
| Cline | Supported | VS Code extension storage |
| Cursor | Experimental | Conversations may be cloud-only |

## Output Formats

Commands support `--format` flag:

```bash
lore sessions --format json    # JSON output
lore show abc123 --format md   # Markdown output
```

## Storage

All data is local:

```
~/.lore/
├── lore.db       # SQLite database
├── config.yaml   # Configuration
└── logs/         # Daemon logs
```

## License

MIT

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

# Lore

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![CI](https://github.com/varalys/lore/actions/workflows/ci.yml/badge.svg)](https://github.com/varalys/lore/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/varalys/lore)](https://github.com/varalys/lore/releases)
![Platform](https://img.shields.io/badge/platform-linux%20%7C%20macOS%20%7C%20WSL2-blue)

**Reasoning history for code.** Lore captures AI coding sessions and links them to git commits.

Git captures what changed, Lore captures why it changed. The prompts, approaches, and decisions from your AI conversations.

**Documentation:** [lore.varalys.com](https://lore.varalys.com)

## Use Cases

- **Code review**: See the AI conversation that produced a PR, not just the diff
- **Debugging**: Understand why code was written a certain way
- **Knowledge transfer**: AI conversations stay with the code when people leave
- **Search**: Find that conversation where you solved a similar problem

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

Download from [GitHub Releases](https://github.com/varalys/lore/releases).

## Quick Start

```bash
# Guided setup
lore init

# Import existing sessions
lore import

# List sessions
lore sessions

# View a session
lore show abc123

# Link to current commit
lore link abc123

# Find sessions for a commit
lore show --commit HEAD

# Trace a line of code to its AI session
lore blame src/main.rs:42

# Search across all sessions
lore search "authentication"
```

## Key Features

| Feature | Description |
|---------|-------------|
| **Session Capture** | Import from 10+ AI coding tools |
| **Git Linking** | Connect sessions to commits |
| **Full-text Search** | Find any conversation |
| **Blame Integration** | Trace code to sessions |
| **MCP Server** | Let AI tools query your history |
| **Background Daemon** | Real-time capture |

## Supported Tools

Claude Code, Codex CLI, Gemini CLI, Amp, Aider, Continue.dev, Cline, Roo Code, Kilo Code, OpenCode

See [Supported Tools](https://lore.varalys.com/reference/supported-tools/) for details.

## MCP Integration

Let Claude Code query your session history:

```bash
claude mcp add lore -- lore mcp serve
```

Claude can then search sessions, retrieve context, and continue where you left off.

See [MCP Guide](https://lore.varalys.com/guides/mcp/) for setup details.

## Documentation

Full documentation at **[lore.varalys.com](https://lore.varalys.com)**:

- [Installation](https://lore.varalys.com/getting-started/installation/)
- [Quick Start](https://lore.varalys.com/getting-started/quick-start/)
- [Command Reference](https://lore.varalys.com/commands/)
- [Guides](https://lore.varalys.com/guides/linking/)
- [FAQ](https://lore.varalys.com/about/faq/)

## Data Location

```
~/.lore/
├── lore.db       # SQLite database
├── config.yaml   # Configuration
└── logs/         # Daemon logs
```

All data stays on your machine.

## License

Apache 2.0

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

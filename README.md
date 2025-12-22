# Lore

**Reasoning history for code** — capture the story behind your commits.

Git tells you *what* changed. Lore tells you *how* and *why* it changed.

## The Problem

In AI-assisted development, the reasoning happens in conversations:

- The prompts you wrote
- The iterations you went through  
- The approaches you rejected
- The context you provided

But git only captures the final code. The story is lost.

## What Lore Does

Lore captures your AI coding sessions and links them to git commits, so you can:

- **Review code with context**: See the reasoning behind a PR, not just the diff
- **Debug with history**: Understand why code was written a certain way
- **Onboard faster**: Learn how your team solves problems
- **Preserve knowledge**: When people move on, the reasoning stays

## Installation

```bash
# macOS
brew install wemfore/tap/lore

# From source
cargo install --path .
```

## Quick Start

```bash
# Import your Claude Code sessions
lore import

# List recent sessions
lore sessions

# View a session
lore show abc123

# Link a session to your current commit
lore link abc123

# View sessions linked to a commit
lore show --commit HEAD
```

## Commands

| Command | Description |
|---------|-------------|
| `lore status` | Show current state and recent sessions |
| `lore sessions` | List and filter sessions |
| `lore show <id>` | View session details |
| `lore show --commit <sha>` | View sessions linked to a commit |
| `lore link <id>` | Link session to HEAD |
| `lore link <id> --commit <sha>` | Link session to specific commit |
| `lore import` | Import sessions from Claude Code |
| `lore config` | View/edit configuration |

## Supported Tools

- ✅ Claude Code
- 🚧 Cursor (planned)
- 🚧 GitHub Copilot (planned)
- 🚧 Windsurf (planned)

## How It Works

1. **Capture**: Lore reads session files from AI coding tools (currently Claude Code's `~/.claude/projects/`)
2. **Store**: Sessions are stored in a local SQLite database (`~/.lore/lore.db`)
3. **Link**: You can link sessions to git commits manually or automatically
4. **View**: Browse reasoning history alongside your code

## Storage

All data is stored locally in `~/.lore/`:

```
~/.lore/
├── lore.db          # SQLite database
└── config.yaml      # Configuration (future)
```

## License

MIT

## Contributing

This is an early-stage project. Contributions welcome!

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5] - 2026-01-08

### Added

- Aider project scanning during `lore init` - detects `.aider.chat.history.md` files and offers to add their directories to watched paths

### Fixed

- Linux systemd service command now uses correct binary path
- Daemon status display on Linux now correctly detects running state
- Aider watcher no longer watches entire home directory when history files are in `~`
- Init UX improved: comma-separated directory input with interactive validation
- Systemd service setup now stops existing daemon first to prevent conflicts
- Reduced log spam for transient database errors during init (logged at DEBUG instead of WARN)

## [0.1.4] - 2026-01-06

### Added

- `lore blame <file:line>` command to trace code back to AI sessions
  - Uses git blame to find the commit that introduced a line
  - Shows linked sessions and relevant message excerpts
  - Supports text (colored), JSON, and Markdown output formats
- `lore export <session-id>` command to export sessions for sharing/archiving
  - Supports Markdown (default) and JSON output formats
  - Includes session metadata, messages, linked commits, tags, and summary
  - `--redact` flag for automatic sensitive content redaction
  - Built-in patterns for API keys, tokens, AWS credentials, emails, IPs, private keys
  - `--redact-pattern <regex>` for custom redaction patterns
  - `-o/--output <file>` to write directly to file

### Fixed

- Session prefix resolution now searches all sessions efficiently (not limited to recent 100-1000)
- Post-commit hook script updated with placeholder documentation

## [0.1.3] - 2026-01-05

### Added

- MCP (Model Context Protocol) server for AI tool integration
- `lore mcp serve` command to start the MCP server on stdio
- Five MCP tools exposed to AI assistants:
  - `lore_search` - search session messages with filters
  - `lore_get_session` - get full session details by ID
  - `lore_list_sessions` - list recent sessions
  - `lore_get_context` - get recent session context for a repository
  - `lore_get_linked_sessions` - get sessions linked to a commit
- Claude Code MCP configuration documentation in README

## [0.1.2] - 2026-01-04

### Added

- `lore current` command to show active session in current directory
- `lore context` command for quick orientation on recent sessions
- `lore context --last` for detailed summary of most recent session
- `lore annotate` command to add notes/bookmarks to sessions
- `lore tag` command to organize sessions with labels
- `lore sessions --tag <label>` to filter sessions by tag
- `lore summarize` command to add/view session summaries
- Machine identity system with UUID for future cloud sync deduplication
- `machine_name` config option for user-friendly display names
- Machine name prompt during `lore init`

### Changed

- `lore config` now displays machine identity (UUID and name)

### Fixed

- `lore daemon stop` now properly handles Homebrew-managed services
- Machine ID migration converts hostname-based IDs to UUIDs

## [0.1.1] - 2026-01-02

### Added

- `lore delete <session-id>` command to permanently remove sessions
- `lore db vacuum` command to reclaim unused database space
- `lore db prune --older-than <duration>` to delete old sessions (supports d/w/m/y)
- `lore db stats` command showing database statistics and tool breakdown
- `lore completions install` for automatic shell completion installation
- Shell completions offered during `lore init` wizard
- Background service installation offered during `lore init` (brew services on macOS, systemd user on Linux)
- Branch history display in `lore sessions` (e.g., `main -> feat/x -> main`)

### Changed

- Daemon now updates session branch when it changes mid-session
- Prune dry-run shows detailed session list matching `lore sessions` format
- Init wizard shows service benefits before prompting

### Fixed

- SIGPIPE panic when piping completions to `head` or other commands
- Branch column overflow with long branch names (now truncated)
- `daemon uninstall` now handles both native and Homebrew-installed services on macOS

## [0.1.0] - 2025-12-30

### Added

- Initial release
- Session capture from Claude Code, Codex CLI, Gemini CLI, Amp, Aider, Continue.dev, Cline, Roo Code, Kilo Code, and OpenCode
- SQLite storage with full-text search (FTS5)
- Manual and automatic session-to-commit linking
- Background daemon with file watching
- System service installation (launchd on macOS, systemd on Linux)
- Git hooks for automatic linking on commit
- CLI commands: status, sessions, show, import, link, unlink, search, config, hooks, daemon
- JSON and Markdown output formats
- GitHub Actions CI and release workflows

[Unreleased]: https://github.com/varalys/lore/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/varalys/lore/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/varalys/lore/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/varalys/lore/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/varalys/lore/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/varalys/lore/releases/tag/v0.1.0

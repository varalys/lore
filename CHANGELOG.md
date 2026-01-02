# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/varalys/lore/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/varalys/lore/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/varalys/lore/releases/tag/v0.1.0

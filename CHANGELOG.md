# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/varalys/lore/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/varalys/lore/releases/tag/v0.1.0

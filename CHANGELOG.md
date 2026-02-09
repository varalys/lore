# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.12] - 2026-02-05

### Added

- `lore insights` command for AI development analytics
  - Commit coverage: repo commits vs commits with linked sessions
  - Tool usage breakdown with session counts and percentages
  - Activity stats: avg duration, avg messages per session, most active day
  - Top files touched across AI sessions
  - `--since` filter with relative (30d, 3m) and absolute (2025-01-01) dates
  - `--repo` to scope insights to a specific repository
  - `--format json` for machine-readable output
- 5 new date-range-aware database query methods for insights analytics

## [0.1.11] - 2026-01-24

### Added

- **Lore Cloud** - Sync sessions across machines with end-to-end encryption
  - `lore login` - Browser-based OAuth authentication
  - `lore logout` - Clear stored credentials and encryption key
  - `lore cloud status` - View account info, sync status, and storage usage
  - `lore cloud push` - Upload sessions to cloud (encrypted)
  - `lore cloud pull` - Download sessions from cloud (decrypted)
  - `lore cloud sync` - Bidirectional sync (pull then push)
- **Client-side encryption** - Your session content is encrypted before upload
  - Passphrase-based key derivation using Argon2id
  - AES-256-GCM encryption for session data
  - Cloud service cannot read your session content
  - Encryption salt synced to cloud for multi-machine support
- **Credential storage options** - Choose how credentials are stored
  - File storage (default) - Simple, works everywhere
  - OS Keychain - macOS Keychain, Windows Credential Manager, Linux Secret Service
- **Daemon automatic cloud sync** - Sessions sync every 4 hours automatically
  - No manual `lore cloud push` needed once configured
  - Sync schedule persists across daemon restarts
  - View next sync time with `lore cloud status`
- **Login encryption setup** - Prompted to set up passphrase after `lore login`
  - Enables immediate auto-sync without separate push command

### Fixed

- Continued sessions (e.g., `claude --continue`) now re-sync with new messages
- Gemini session deduplication - Multiple files with same session ID no longer cause repeated syncs
- Cloud salt error handling - Network errors no longer accidentally create new encryption salt

## [0.1.10] - 2026-01-13

### Added

- `lore doctor` command for diagnosing installation and configuration issues
  - Checks config, database, daemon, watchers, and MCP server
  - Supports text and JSON output formats
  - Exit codes: 0 (OK), 1 (warnings), 2 (errors)
- `lore link --auto` re-enabled with preview-first UX
  - Shows proposed links and requires `--yes` to apply
  - Uses heuristics: time proximity, file overlap, branch matching
- `lore link --auto --backfill` for bulk retroactive linking
  - Scans all ended sessions and links commits from their time windows
  - One-time migration tool for existing session history

### Fixed

- Path matching in `find_active_sessions_for_directory` now avoids prefix collisions
  - `/project` no longer incorrectly matches `/project-old`
- macOS launchd now treats "service already loaded" as success
  - Prevents duplicate daemon spawns when service is already running

## [0.1.9] - 2026-01-12

### Fixed

- `lore daemon start` now uses systemctl/launchctl when a service file exists, matching `lore daemon stop` behavior
  - Previously, `daemon start` would spawn a standalone process even if a systemd/launchd service was installed
  - This caused confusion where the daemon ran outside of service manager control

## [0.1.8] - 2026-01-12

### Added

- Forward auto-linking: daemon automatically links sessions to commits when sessions end
  - No git hooks or per-repo setup required
  - Finds commits across all branches made during session time window
- `lore link --current` flag to manually link active sessions to HEAD
- `get_commits_in_time_range()` for multi-branch commit discovery

### Fixed

- Daemon now re-imports updated sessions and triggers auto-linking on updates
- Fixed incorrect watcher dispatch that caused Claude JSONL files to be parsed as aider sessions
  - Now uses path-based dispatch to match files to their owning watcher
- Fixed daemon logging - logs are now written to `~/.lore/daemon.log`
  - Previously, console logging initialization prevented file logging from initializing

## [0.1.7] - 2026-01-10

### Added

- Daemon version check in `lore status` warns when CLI and daemon versions differ after upgrades
- Homebrew formula now uses prebuilt binaries for instant installation (no cargo build)
- Fallback to native launchd service when Homebrew is unavailable on macOS

### Changed

- Homebrew caveats now clearly warn that `lore init` must be run before the service will work
- Release workflow automatically generates Homebrew formula with correct binary URLs and SHAs

## [0.1.6] - 2026-01-09

### Fixed

- Systemd service file now uses dynamic binary path detection via `current_exe()` instead of hardcoded `~/.cargo/bin/lore`, fixing service startup failures when installed via package managers (AUR, etc.)

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

[Unreleased]: https://github.com/varalys/lore/compare/v0.1.10...HEAD
[0.1.10]: https://github.com/varalys/lore/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/varalys/lore/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/varalys/lore/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/varalys/lore/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/varalys/lore/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/varalys/lore/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/varalys/lore/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/varalys/lore/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/varalys/lore/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/varalys/lore/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/varalys/lore/releases/tag/v0.1.0

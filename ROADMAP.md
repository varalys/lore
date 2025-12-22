# Lore Development Roadmap

This document contains the structured task list for Lore development. Tasks are organized by phase and priority. Update status as work progresses.

## Status Legend

- `[ ]` Not started
- `[~]` In progress
- `[x]` Complete
- `[!]` Blocked

---

## Phase 0: Foundation and Testing

Before adding new features, establish a solid testing foundation for existing code.

### 0.1 Storage Layer Tests
- [x] Unit tests for Session CRUD operations in db.rs
- [x] Unit tests for Message CRUD operations in db.rs
- [x] Unit tests for SessionLink CRUD operations in db.rs
- [x] Test database migrations run correctly
- [ ] Test concurrent read operations

### 0.2 Claude Code Parser Tests
- [x] Unit tests for JSONL parsing (valid input)
- [x] Unit tests for malformed JSONL handling
- [x] Unit tests for message type parsing (human, assistant, tool_use, tool_result)
- [x] Unit tests for session file discovery
- [x] Integration test: parse real session file format

### 0.3 CLI Command Tests
- [x] Integration tests for `lore sessions` command
- [x] Integration tests for `lore show` command
- [x] Integration tests for `lore import` command
- [x] Integration tests for `lore link` command
- [x] Test error handling and user-friendly messages

### 0.4 Code Quality
- [x] Run clippy and fix all warnings
- [x] Add doc comments to all public functions
- [x] Add module-level documentation to all modules
- [x] Ensure consistent error handling patterns

---

## Phase 1: Core CLI Completion

Complete the CLI commands that are currently stubs or incomplete.

### 1.1 Status Command Enhancement
- [ ] Show daemon status (running/stopped) placeholder
- [ ] Show active watchers
- [ ] Show recent session summary
- [ ] Show sessions linked to HEAD commit
- [ ] Display storage statistics

### 1.2 Search Command Implementation
- [ ] Design search index strategy (SQLite FTS5 vs separate index)
- [ ] Implement full-text indexing of message content
- [ ] Implement `lore search <query>` basic functionality
- [ ] Add filtering options (--repo, --since, --tool)
- [ ] Add result formatting and pagination

### 1.3 Unlink Command Implementation
- [ ] Implement session-commit unlinking
- [ ] Add confirmation prompt for destructive action
- [ ] Support unlinking by session ID or commit SHA

### 1.4 Config Command Enhancement
- [ ] Design configuration file format (YAML)
- [ ] Implement config file loading and saving
- [ ] Add `lore config set` command
- [ ] Add `lore config get` command
- [ ] Support both global and repo-level config

---

## Phase 2: Git Integration

Improve the connection between sessions and git history.

### 2.1 Auto-linking Heuristics
- [ ] Implement time-based session matching (sessions active near commit time)
- [ ] Implement file-based session matching (sessions that touched committed files)
- [ ] Implement confidence scoring algorithm
- [ ] Add `lore link --auto` command
- [ ] Add configurable auto-link threshold

### 2.2 Git Hooks
- [ ] Create post-commit hook template
- [ ] Implement `lore hooks install` command
- [ ] Implement `lore hooks uninstall` command
- [ ] Add prepare-commit-msg hook for session references

### 2.3 Show by Commit
- [ ] Enhance `lore show --commit <sha>` to show linked sessions
- [ ] Support partial SHA matching
- [ ] Support HEAD and other refs

---

## Phase 3: Background Daemon

Enable automatic session capture without manual import.

### 3.1 Daemon Infrastructure
- [ ] Add tokio dependency for async runtime
- [ ] Add notify dependency for file watching
- [ ] Design daemon process management (start/stop/status)
- [ ] Implement Unix socket IPC for CLI communication

### 3.2 File Watcher
- [ ] Implement directory watcher for Claude Code sessions
- [ ] Handle file creation, modification, and deletion events
- [ ] Implement incremental parsing (track read positions)
- [ ] Handle session boundaries (when does a session "end"?)

### 3.3 Daemon Commands
- [ ] Implement `lore daemon start`
- [ ] Implement `lore daemon stop`
- [ ] Implement `lore daemon status`
- [ ] Implement `lore daemon logs`
- [ ] Add auto-start configuration option

---

## Phase 4: Additional Watchers

Expand capture beyond Claude Code.

### 4.1 Watcher Abstraction
- [ ] Define Watcher trait for common interface
- [ ] Refactor claude_code.rs to implement trait
- [ ] Add watcher registration and discovery

### 4.2 Cursor Watcher
- [ ] Research Cursor session storage format
- [ ] Implement SQLite state.vscdb parser
- [ ] Handle schema version differences
- [ ] Add Cursor watcher to daemon

### 4.3 Generic MCP Watcher (Future)
- [ ] Research MCP protocol for session capture
- [ ] Design MCP-based capture approach

---

## Phase 5: Polish and Distribution

Prepare for public release.

### 5.1 Output Formatting
- [ ] Implement JSON output format for all commands
- [ ] Implement markdown output format for show command
- [ ] Add --format flag to relevant commands
- [ ] Ensure consistent column alignment in table output

### 5.2 Error Messages
- [ ] Audit all error messages for clarity
- [ ] Add helpful suggestions in error output
- [ ] Ensure no panics reach user (graceful error handling)

### 5.3 Documentation
- [ ] Write README.md with installation and usage
- [ ] Add man page or --help improvements
- [ ] Create CONTRIBUTING.md for open source

### 5.4 Distribution
- [ ] Create release builds for macOS (arm64, x86_64)
- [ ] Create release builds for Linux
- [ ] Create Homebrew formula
- [ ] Set up GitHub releases

---

## Backlog (Future Phases)

Items for consideration after MVP.

### Cloud Sync
- [ ] User accounts and authentication
- [ ] Session sync protocol
- [ ] Conflict resolution
- [ ] Encryption at rest

### Team Features
- [ ] Session sharing permissions
- [ ] Team dashboard
- [ ] GitHub/GitLab PR integration

### Additional Integrations
- [ ] VS Code extension
- [ ] GitHub Copilot watcher
- [ ] Windsurf watcher

---

## Notes

### Dependencies to Add (when needed)
- `notify = "6"` for file watching (Phase 3)
- `tokio = { version = "1", features = ["full"] }` for daemon (Phase 3)

### Design Decisions to Make
- Search index: SQLite FTS5 vs tantivy vs separate index
- Config format: YAML vs TOML
- Daemon IPC: Unix socket vs named pipe vs HTTP

### Technical Debt
- Dead code in git/mod.rs (repo_info, calculate_link_confidence)
- Dead code in config/mod.rs (load, config_path)
- Dead code in storage/models.rs (summary, text methods)
- Unused fields in claude_code.rs parser (agent_id, signature)

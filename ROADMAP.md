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
- [x] Show daemon status (running/stopped) placeholder
- [x] Show active watchers
- [x] Show recent session summary
- [x] Show sessions linked to HEAD commit
- [x] Display storage statistics

### 1.2 Search Command Implementation
- [x] Design search index strategy (SQLite FTS5 vs separate index)
- [x] Implement full-text indexing of message content
- [x] Implement `lore search <query>` basic functionality
- [x] Add filtering options (--repo, --since, --role)
- [x] Add result formatting and pagination

### 1.3 Unlink Command Implementation
- [x] Implement session-commit unlinking
- [x] Add confirmation prompt for destructive action
- [x] Support unlinking by session ID or commit SHA

### 1.4 Config Command Enhancement
- [x] Design configuration file format (YAML)
- [x] Implement config file loading and saving
- [x] Add `lore config set` command
- [x] Add `lore config get` command
- [ ] Support both global and repo-level config (deferred to future)

---

## Phase 2: Git Integration

Improve the connection between sessions and git history.

### 2.1 Auto-linking Heuristics
- [x] Implement time-based session matching (sessions active near commit time)
- [x] Implement file-based session matching (sessions that touched committed files)
- [x] Implement confidence scoring algorithm
- [x] Add `lore link --auto` command
- [x] Add configurable auto-link threshold

### 2.2 Git Hooks
- [x] Create post-commit hook template
- [x] Implement `lore hooks install` command
- [x] Implement `lore hooks uninstall` command
- [x] Add prepare-commit-msg hook for session references

### 2.3 Show by Commit
- [x] Enhance `lore show --commit <sha>` to show linked sessions
- [x] Support partial SHA matching
- [x] Support HEAD and other refs

---

## Phase 3: Background Daemon

Enable automatic session capture without manual import.

### 3.1 Daemon Infrastructure
- [x] Add tokio dependency for async runtime
- [x] Add notify dependency for file watching
- [x] Design daemon process management (start/stop/status)
- [x] Implement Unix socket IPC for CLI communication

### 3.2 File Watcher
- [x] Implement directory watcher for Claude Code sessions
- [x] Handle file creation, modification, and deletion events
- [x] Implement incremental parsing (track read positions)
- [x] Handle session boundaries (when does a session "end"?)

### 3.3 Daemon Commands
- [x] Implement `lore daemon start`
- [x] Implement `lore daemon stop`
- [x] Implement `lore daemon status`
- [x] Implement `lore daemon logs`
- [ ] Add auto-start configuration option (deferred)

---

## Phase 4: Additional Watchers

Expand capture beyond Claude Code.

### 4.1 Watcher Abstraction
- [x] Define Watcher trait for common interface
- [x] Refactor claude_code.rs to implement trait
- [x] Add watcher registration and discovery

### 4.2 Aider Watcher
- [x] Research Aider chat history format (.aider.chat.history.md)
- [x] Implement markdown parser for chat history
- [x] Add Aider watcher to registry

### 4.3 Continue.dev Watcher
- [x] Research Continue.dev session storage (~/.continue/sessions/)
- [x] Implement JSON session parser
- [x] Add Continue.dev watcher to registry

### 4.4 Cline Watcher
- [x] Research Cline (Claude Dev) storage format
- [x] Implement JSON conversation parser
- [x] Add Cline watcher to registry

### 4.5 Codex CLI Watcher
- [x] Research Codex CLI session storage (~/.codex/sessions/)
- [x] Implement JSONL parser
- [x] Add Codex watcher to registry

### 4.6 Gemini CLI Watcher
- [x] Research Gemini CLI session storage (~/.gemini/tmp/)
- [x] Implement JSON parser
- [x] Add Gemini watcher to registry

### 4.7 Amp Watcher
- [x] Research Amp session storage (~/.local/share/amp/threads/)
- [x] Implement JSON parser with thinking block support
- [x] Add Amp watcher to registry

### 4.8 OpenCode Watcher
- [x] Research OpenCode session storage (~/.local/share/opencode/storage/)
- [x] Implement multi-file JSON parser (session/message/part structure)
- [x] Add OpenCode watcher to registry

### 4.9 Roo Code Watcher
- [x] Research Roo Code storage (VS Code extension, fork of Cline)
- [x] Implement JSON conversation parser
- [x] Add Roo Code watcher to registry

### 4.10 Generic MCP Watcher (Future)
- [ ] Research MCP protocol for session capture
- [ ] Design MCP-based capture approach

### 4.11 Cursor Watcher (Blocked)
- [x] Research Cursor session storage format
- [!] Conversations synced to cloud, not stored locally - removed from watchers

---

## Phase 5: Polish and Distribution

Prepare for public release.

### 5.1 Output Formatting
- [x] Implement JSON output format for all commands
- [x] Implement markdown output format for show command
- [x] Add --format flag to relevant commands
- [x] Ensure consistent column alignment in table output

### 5.2 Error Messages
- [x] Audit all error messages for clarity
- [x] Add helpful suggestions in error output
- [x] Ensure no panics reach user (graceful error handling)

### 5.3 Documentation
- [x] Write README.md with installation and usage
- [x] Add man page or --help improvements
- [x] Create CONTRIBUTING.md for open source

### 5.4 Distribution
- [x] Create release builds for macOS (arm64, x86_64)
- [x] Create release builds for Linux
- [x] Set up GitHub releases (GitHub Actions workflow)
- [ ] Create Homebrew formula (deferred until post-release)

---

## Phase 6: Cloud Sync Foundation

Enable users to sync sessions to cloud storage. Free tier gets basic sync; paid teams get encryption.

### 6.1 Authentication
- [ ] Add `lore auth login` command (opens browser for OAuth)
- [ ] Add `lore auth logout` command
- [ ] Add `lore auth status` command
- [ ] Store credentials in `~/.lore/credentials`
- [ ] Token refresh handling

### 6.2 Sync Protocol Design
- [ ] Define sync API contract (REST endpoints in lore-cloud)
- [ ] Design session upload format (JSON payload)
- [ ] Design incremental sync (track last sync timestamp per session)
- [ ] Handle offline-first with queue for pending uploads
- [ ] Define rate limits and quotas per tier

### 6.3 Sync Implementation
- [ ] Implement session upload to cloud API
- [ ] Add `lore sync` manual sync command
- [ ] Add background sync in daemon (when authenticated)
- [ ] Add sync status to `lore status` output
- [ ] Add `--no-sync` flag for sensitive sessions
- [ ] First-sync onboarding prompt (explain what syncs, offer encryption)

### 6.4 Encryption (Paid Team Feature)
- [ ] Generate user keypair on first team join
- [ ] Encrypt message content client-side (AES-256-GCM)
- [ ] Per-session keys wrapped for team members' public keys
- [ ] Key storage and backup flow
- [ ] Decrypt on display (`lore show` fetches and decrypts)

---

## Phase 7: Enterprise Features

Team collaboration features (requires lore-cloud web app).

### 7.1 Team Accounts
- [ ] Organization/team creation in web app
- [ ] Invite team members
- [ ] API key scoped to organization

### 7.2 Session Sharing
- [ ] Share sessions with team (sync to shared workspace)
- [ ] Permissions (view-only, admin)
- [ ] Session visibility controls (private, team, public)

### 7.3 PR Integration
- [ ] GitHub App for PR comments with linked sessions
- [ ] GitLab integration
- [ ] Link sessions in PR description automatically

### 7.4 Web Dashboard
- [ ] View synced sessions in browser
- [ ] Search across team sessions
- [ ] Session analytics (usage patterns, tool breakdown)

---

## Phase 8: Additional Integrations

### 8.1 VS Code Extension
- [ ] Show linked sessions in editor
- [ ] Quick link current session to commit
- [ ] Session browser panel

### 8.2 Additional Watchers
- [ ] GitHub Copilot (likely cloud-only, needs investigation)
- [ ] Windsurf (Codeium-based, investigate storage format)
- [ ] Sourcegraph Cody (investigate storage format)
- [ ] Amazon Q Developer (investigate storage format)
- [ ] Tabnine (investigate storage format)
- [ ] Cursor improvements (reverse engineer cloud API or monitor traffic)

---

## Notes

### Dependencies to Add (when needed)
- `notify = "6"` for file watching (Phase 3)
- `tokio = { version = "1", features = ["full"] }` for daemon (Phase 3)

### Design Decisions to Make
- Search index: SQLite FTS5 vs tantivy vs separate index
- Config format: YAML vs TOML
- Daemon IPC: Unix socket vs named pipe vs HTTP

### Cloud Architecture Decisions (Phase 6+) - DECIDED

- **Cloud storage**: Turso (SQLite-compatible, works with our schema)
- **Web app repo**: Separate `lore-cloud` repo for web dashboard, API, billing
- **Auth flow**: OAuth via browser
  - `lore auth login` opens browser to web app
  - User authenticates (GitHub, Google, or email)
  - CLI receives token, stores in `~/.lore/credentials`
- **Sync model**: Push from CLI to cloud API
- **Encryption**: Zero-knowledge E2E encryption (paid team feature)
  - Metadata (session ID, timestamps, tool, directory) unencrypted for indexing
  - Message content encrypted client-side before upload
  - First-sync prompt asks user to enable encryption
  - We cannot read customer session content
  - Per-session keys wrapped for team members (enables sharing)

### Technical Debt
- Dead code in git/mod.rs (repo_info, calculate_link_confidence)
- Dead code in config/mod.rs (load, config_path)
- Dead code in storage/models.rs (summary, text methods)
- Unused fields in claude_code.rs parser (agent_id, signature)

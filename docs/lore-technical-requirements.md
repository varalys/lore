# Lore: Technical Requirements Document

## Overview

Lore is a reasoning history system that captures, stores, links, and surfaces the AI-assisted development sessions that produce code. This document defines the technical architecture and requirements for the MVP and future iterations.

---

## 1. Core Data Model

### 1.1 Session

A **Session** is the primary unit of reasoning history—a complete record of a human-AI collaboration.

```
Session {
  id: UUID
  tool: string                    // "claude-code", "cursor", "copilot", etc.
  started_at: timestamp
  ended_at: timestamp | null      // null if ongoing
  model: string | null            // "claude-sonnet-4-20250514", "gpt-4", etc.
  working_directory: string       // repo/project path
  
  messages: Message[]             // ordered conversation
  tool_calls: ToolCall[]          // actions taken by AI
  files_touched: string[]         // files read/written during session
  
  metadata: {
    tool_version: string
    session_type: string          // "chat", "agent", "inline", etc.
    custom: Record<string, any>   // tool-specific data
  }
}
```

### 1.2 Message

```
Message {
  id: UUID
  session_id: UUID
  index: number                   // position in conversation
  timestamp: timestamp
  role: "human" | "assistant" | "system" | "tool_result"
  content: string
  
  // For assistant messages
  model: string | null
  stop_reason: string | null      // "end_turn", "tool_use", etc.
  
  // For tool results
  tool_call_id: UUID | null
}
```

### 1.3 ToolCall

```
ToolCall {
  id: UUID
  session_id: UUID
  message_id: UUID                // which assistant message triggered this
  timestamp: timestamp
  
  tool_name: string               // "str_replace", "bash", "web_search", etc.
  input: Record<string, any>      // tool parameters
  output: string | null           // tool result (may be truncated)
  
  // For file operations
  file_path: string | null
  diff: string | null             // for edits, the actual change
}
```

### 1.4 SessionLink

Links sessions to git commits/refs.

```
SessionLink {
  id: UUID
  session_id: UUID
  
  link_type: "commit" | "branch" | "pr" | "manual"
  
  // Git reference
  commit_sha: string | null
  branch: string | null
  remote: string | null           // "origin", etc.
  
  // PR reference (future)
  pr_number: number | null
  pr_url: string | null
  
  created_at: timestamp
  created_by: "auto" | "user"
  confidence: number | null       // for auto-links, 0.0-1.0
}
```

### 1.5 Repository

```
Repository {
  id: UUID
  path: string                    // absolute path on disk
  name: string                    // derived from path or .git
  remote_url: string | null       // origin URL if available
  
  created_at: timestamp
  last_session_at: timestamp
}
```

---

## 2. Storage Architecture

### 2.1 MVP: Local SQLite

For MVP, all data stored in a single SQLite database per machine.

**Location**: `~/.lore/lore.db`

**Rationale**:
- Zero infrastructure for users to set up
- Fast queries for local operations
- Easy to back up (single file)
- SQLite handles concurrent reads well
- Matches how Claude Code, Cursor already store data

**Schema migrations**: Use simple versioned SQL files, applied on CLI startup.

### 2.2 Future: Local + Cloud Sync

Post-MVP architecture:

```
┌─────────────────┐     ┌─────────────────┐
│  Local SQLite   │────▶│   Sync Service  │
│  (~/.lore/)     │◀────│                 │
└─────────────────┘     └────────┬────────┘
                                 │
                                 ▼
                        ┌─────────────────┐
                        │  Cloud Storage  │
                        │  (per-team)     │
                        └─────────────────┘
```

Considerations:
- Conflict resolution for same session edited on multiple machines
- Selective sync (don't sync sessions marked private)
- Encryption at rest for cloud storage
- Team/org boundaries

---

## 3. Capture System

### 3.1 Capture Architecture

```
┌──────────────────────────────────────────────────────────┐
│                     Lore Daemon                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐      │
│  │ Claude Code │  │   Cursor    │  │   Copilot   │ ...  │
│  │   Watcher   │  │   Watcher   │  │   Watcher   │      │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘      │
│         │                │                │              │
│         ▼                ▼                ▼              │
│  ┌──────────────────────────────────────────────────┐   │
│  │              Unified Session Parser               │   │
│  └──────────────────────────────────────────────────┘   │
│                          │                               │
│                          ▼                               │
│  ┌──────────────────────────────────────────────────┐   │
│  │              SQLite Storage Layer                 │   │
│  └──────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

### 3.2 MVP: Claude Code Watcher

Start with Claude Code only. It stores sessions in a transparent format.

**Source location**: `~/.claude/projects/<project-hash>/sessions/`

**File format**: JSONL (one JSON object per line)

**Watcher behavior**:
1. On startup, scan for existing session files
2. Watch directory for new/modified files (fsnotify or similar)
3. Parse JSONL incrementally (track last-read position)
4. Transform to Lore's Session model
5. Store in SQLite

**Claude Code JSONL structure** (observed):
```jsonl
{"type":"human","message":{"role":"user","content":"..."},"timestamp":"...","sessionId":"..."}
{"type":"assistant","message":{"role":"assistant","content":"...","stop_reason":"..."},"timestamp":"...","sessionId":"..."}
{"type":"tool_use","tool":{"name":"str_replace","input":{...}},"timestamp":"...","sessionId":"..."}
{"type":"tool_result","result":"...","timestamp":"...","sessionId":"..."}
```

### 3.3 Future: Additional Watchers

**Cursor**:
- Location: `~/.cursor/` or workspace `.cursor/`
- Format: SQLite (`state.vscdb`)
- Challenge: Schema changes between versions, undocumented structure

**GitHub Copilot**:
- No local storage by default
- Would require VS Code extension to intercept
- Or: parse VS Code telemetry/logs if available

**Windsurf**:
- Stores up to 20 conversations
- Format: Unknown, needs investigation

**Generic approach**: For tools without accessible storage, consider:
- Browser extension (for web-based tools)
- VS Code extension (intercepts AI interactions)
- Proxy approach (intercept API calls)

---

## 4. Git Integration

### 4.1 Automatic Linking

When a commit is made, Lore attempts to automatically associate relevant sessions.

**Heuristics for auto-linking**:

1. **Time overlap**: Sessions active within N minutes of commit
2. **File overlap**: Sessions that touched files in the commit
3. **Working directory**: Sessions in the same repo

**Algorithm**:
```
on git commit detected:
  commit_files = files changed in commit
  commit_time = commit timestamp
  repo_path = repo root
  
  candidate_sessions = sessions where:
    - working_directory matches repo_path
    - ended_at within 30 minutes of commit_time (configurable)
    - OR files_touched intersects commit_files
  
  for each candidate:
    score = calculate_relevance(session, commit)
    if score > threshold:
      create SessionLink(session, commit, confidence=score)
```

**Confidence scoring**:
- High file overlap: +0.4
- Recent timing: +0.3
- Same branch: +0.2
- Manual confirmation: 1.0

### 4.2 Git Hooks Integration

Optional git hooks to enhance capture:

**post-commit hook**:
```bash
#!/bin/bash
lore link --auto --commit HEAD
```

**prepare-commit-msg hook** (future):
```bash
#!/bin/bash
# Append session references to commit message
lore suggest-sessions >> "$1"
```

### 4.3 Commit Message Footer

When sessions are linked, optionally append to commit message:

```
fix: resolve race condition in queue processor

Lore-Sessions: abc123, def456
```

Configurable via `lore config set commit-footer true`

---

## 5. CLI Design

### 5.1 Command Structure

```
lore <command> [subcommand] [options]
```

### 5.2 Core Commands

#### `lore status`
Show current capture status and recent sessions.

```
$ lore status

Lore v0.1.0
Daemon: running (pid 12345)
Watchers: claude-code (active)

Recent sessions:
  abc123  2 hours ago   45 messages   ~/projects/myapp
  def456  yesterday     12 messages   ~/projects/myapp
  
Linked to HEAD (a]1b2c3d):
  abc123 (auto, confidence: 0.85)
```

#### `lore sessions`
List and filter sessions.

```
$ lore sessions                           # list recent
$ lore sessions --repo .                  # sessions for current repo
$ lore sessions --since "3 days ago"      # time filter
$ lore sessions --search "authentication" # search content
$ lore sessions --unlinked                # sessions not linked to any commit
```

#### `lore show <session-id | commit>`
Display session details or sessions linked to a commit.

```
$ lore show abc123                        # show specific session
$ lore show HEAD                          # show sessions linked to HEAD
$ lore show --commit a1b2c3d              # show sessions linked to commit

# Output
Session abc123
Tool: claude-code
Started: 2025-01-15 14:30:00
Duration: 45 minutes
Messages: 45
Files touched: src/auth.ts, src/middleware.ts

[Human 14:30:00]
I need to add rate limiting to the auth endpoint...

[Assistant 14:30:15]
I'll help you implement rate limiting. Let me first look at...

[Tool: view src/auth.ts]
...
```

#### `lore link <session-id> [--commit <ref>]`
Manually link a session to a commit.

```
$ lore link abc123                        # link to HEAD
$ lore link abc123 --commit a1b2c3d       # link to specific commit
$ lore link abc123 def456 --commit HEAD   # link multiple sessions
```

#### `lore unlink <session-id> [--commit <ref>]`
Remove a session link.

```
$ lore unlink abc123 --commit HEAD
```

#### `lore search <query>`
Full-text search across sessions.

```
$ lore search "rate limiting"
$ lore search "rate limiting" --repo .
$ lore search "authentication" --since "last week"
```

#### `lore daemon`
Manage the background capture daemon.

```
$ lore daemon start
$ lore daemon stop
$ lore daemon status
$ lore daemon logs
```

#### `lore config`
Manage configuration.

```
$ lore config list
$ lore config set <key> <value>
$ lore config get <key>

# Example settings
auto-link: true
auto-link-threshold: 0.7
commit-footer: false
watchers: ["claude-code"]
```

#### `lore init`
Initialize Lore for a repository (optional, enables repo-specific config).

```
$ cd myproject
$ lore init

Created .lore/config.yaml
Added .lore to .gitignore
```

### 5.3 Output Formats

Support multiple output formats for scripting:

```
$ lore sessions --format json
$ lore sessions --format table           # default
$ lore show abc123 --format markdown
```

---

## 6. Configuration

### 6.1 Global Config

Location: `~/.lore/config.yaml`

```yaml
# Daemon settings
daemon:
  auto_start: true
  log_level: info

# Capture settings
capture:
  watchers:
    - claude-code
    # - cursor  (future)
    # - copilot (future)
  
# Git integration
git:
  auto_link: true
  auto_link_threshold: 0.7
  commit_footer: false
  
# Storage
storage:
  database: ~/.lore/lore.db
  max_session_age_days: 365    # auto-cleanup old sessions
  
# Privacy
privacy:
  redact_patterns:             # regex patterns to redact from stored content
    - "sk-[a-zA-Z0-9]{48}"     # OpenAI API keys
    - "AKIA[A-Z0-9]{16}"       # AWS access keys
```

### 6.2 Repository Config

Location: `<repo>/.lore/config.yaml`

```yaml
# Override global settings for this repo
git:
  auto_link: true
  
# Sessions in this repo default to team-visible (future)
sharing:
  default: team
```

---

## 7. Technology Choices

### 7.1 Language: Rust

**Rationale**:
- Fast startup time (important for CLI)
- Low memory footprint for daemon
- Excellent SQLite bindings (rusqlite)
- Good file watching support (notify crate)
- Single binary distribution
- Memory safety for long-running daemon

**Alternatives considered**:
- Go: Also good, but Rust's enum types better fit the data model
- TypeScript/Node: Slower startup, heavier runtime
- Python: Slower, distribution more complex

### 7.2 Key Dependencies

```toml
[dependencies]
# CLI
clap = "4"                    # argument parsing
indicatif = "0.17"            # progress bars, spinners

# Storage
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# File watching
notify = "6"                  # cross-platform file watcher

# Git
git2 = "0.18"                 # libgit2 bindings

# Async (for daemon)
tokio = { version = "1", features = ["full"] }

# Utilities
chrono = "0.4"
uuid = { version = "1", features = ["v4"] }
thiserror = "1"
tracing = "0.1"               # logging
```

### 7.3 Project Structure

```
lore/
├── Cargo.toml
├── src/
│   ├── main.rs               # CLI entry point
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── commands/
│   │   │   ├── status.rs
│   │   │   ├── sessions.rs
│   │   │   ├── show.rs
│   │   │   ├── link.rs
│   │   │   ├── search.rs
│   │   │   ├── daemon.rs
│   │   │   └── config.rs
│   │   └── output.rs         # formatting (table, json, markdown)
│   ├── daemon/
│   │   ├── mod.rs
│   │   ├── server.rs         # IPC for CLI communication
│   │   └── watchers/
│   │       ├── mod.rs
│   │       ├── claude_code.rs
│   │       └── base.rs       # watcher trait
│   ├── storage/
│   │   ├── mod.rs
│   │   ├── db.rs             # SQLite operations
│   │   ├── migrations/
│   │   └── models.rs
│   ├── git/
│   │   ├── mod.rs
│   │   ├── hooks.rs
│   │   └── linking.rs        # auto-link logic
│   └── config/
│       └── mod.rs
└── tests/
```

---

## 8. MVP Milestones

### Milestone 1: Core Storage & CLI (Week 1-2)

- [ ] SQLite schema and migrations
- [ ] Basic data models (Session, Message, ToolCall)
- [ ] `lore sessions` command (list from test data)
- [ ] `lore show <session>` command
- [ ] `lore config` command

### Milestone 2: Claude Code Capture (Week 3-4)

- [ ] Claude Code JSONL parser
- [ ] File watcher for session directory
- [ ] Daemon process (start/stop/status)
- [ ] Incremental parsing (don't re-parse entire files)
- [ ] `lore status` command

### Milestone 3: Git Integration (Week 5-6)

- [ ] SessionLink model and storage
- [ ] `lore link` command (manual linking)
- [ ] `lore show <commit>` (show linked sessions)
- [ ] Auto-linking heuristics
- [ ] Git hooks installation helper

### Milestone 4: Search & Polish (Week 7-8)

- [ ] Full-text search index
- [ ] `lore search` command
- [ ] Output formatting (table, json, markdown)
- [ ] Documentation
- [ ] Homebrew formula / install script

---

## 9. Future Considerations

### 9.1 Additional Watchers
- Cursor (SQLite parsing)
- Copilot (VS Code extension)
- Windsurf
- Generic MCP-based capture

### 9.2 Cloud Sync
- User accounts
- Team/org structure
- Sync protocol
- Conflict resolution
- End-to-end encryption

### 9.3 GitHub/GitLab Integration
- PR comments with session links
- Web UI for viewing sessions
- OAuth for authentication

### 9.4 IDE Extensions
- VS Code extension for in-editor session viewing
- JetBrains plugin

### 9.5 Analytics
- Team-level insights on AI usage
- Effective prompt patterns
- Time saved metrics

---

## 10. Security Considerations

### 10.1 Sensitive Data in Sessions

Sessions may contain:
- API keys accidentally pasted
- Proprietary code/logic
- Personal information

**Mitigations**:
- Configurable redaction patterns
- Local-first storage (data doesn't leave machine by default)
- Encryption at rest (future)
- Session-level privacy controls

### 10.2 Daemon Security

- Daemon runs as user, not root
- IPC via Unix socket with user-only permissions
- No network listening in MVP

### 10.3 Git Hook Security

- Hooks are opt-in
- No automatic execution of remote code
- Hooks are local scripts user can inspect

---

## 11. Open Technical Questions

1. **Session boundaries**: How do we detect when a session "ends" vs is just paused? Timeout-based? Tool-specific signals?

2. **Large sessions**: Some sessions may have hundreds of messages. How do we handle display/storage efficiently?

3. **Diff storage**: Should we store full tool outputs or just references? Full file contents or just diffs?

4. **Cross-machine identity**: When cloud sync is added, how do we handle same session accessed from multiple machines?

5. **Real-time vs batch**: Should linking happen in real-time (daemon watches for commits) or batch (on CLI invocation)?

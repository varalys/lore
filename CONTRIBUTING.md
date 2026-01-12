# Contributing to Lore

Thank you for your interest in contributing to Lore! This document provides guidelines and instructions for contributing.

## Getting Started

### Prerequisites

- Rust 1.92 or later
- Git

### Development Setup

```bash
# Clone the repository
git clone https://github.com/varalys/lore.git
cd lore

# Build the project
cargo build

# Run tests
cargo test

# Run the CLI
cargo run -- --help
```

## Development Workflow

### Branch Strategy

1. Create a feature branch from `main`:
   ```bash
   git checkout -b feat/your-feature-name
   ```

2. Make your changes with clear, focused commits

3. Ensure all checks pass:
   ```bash
   cargo check
   cargo clippy --all-targets
   cargo test
   ```

4. Submit a pull request to `main`

### Code Quality Requirements

Before submitting a PR, ensure:

- **No compiler errors**: `cargo check` passes
- **No warnings**: `cargo clippy --all-targets` has zero warnings
- **All tests pass**: `cargo test` succeeds
- **Code is formatted**: `cargo fmt` applied

### Commit Messages

Use clear, descriptive commit messages:

```
feat: add support for Windsurf sessions
fix: handle empty session files gracefully
docs: update README with new commands
test: add integration tests for search
refactor: extract common parsing logic
```

## Code Style

### Rust Guidelines

- Follow standard Rust conventions and idioms
- Use `rustfmt` for formatting
- Use `thiserror` for custom error types
- Use `anyhow` for application error handling
- Add doc comments (`///`) to public items
- Keep functions focused and small

### Documentation

- Add doc comments to all public functions, structs, and modules
- Keep comments concise and focused on "why" not "what"
- Update README.md if adding user-facing features

### Testing

- Write unit tests for new functions
- Test edge cases and error conditions
- Ensure tests are deterministic (no flaky tests)

## Adding a New Watcher

### For Tool Creators

If you build an AI coding tool and want Lore to support it, open an issue with:

- **Tool name and website**
- **Storage location**: Where session files are stored (e.g., `~/.yourtool/sessions/`)
- **File format**: JSON, JSONL, Markdown, SQLite, etc.
- **Schema documentation** or example session files (sanitized of sensitive data)

We can help build the watcher, or you can submit a PR yourself.

### For Contributors

There are two approaches depending on the tool type:

#### Option 1: VS Code Extension (Cline-style)

If the tool is a VS Code extension using Cline-style task storage (with `api_conversation_history.json` files), use the generic `VsCodeExtensionWatcher`:

```rust
// src/capture/watchers/my_extension.rs
use super::vscode_extension::{VsCodeExtensionConfig, VsCodeExtensionWatcher};

pub const CONFIG: VsCodeExtensionConfig = VsCodeExtensionConfig {
    name: "my-extension",
    description: "My Extension VS Code sessions",
    extension_id: "publisher.my-extension",
};

pub fn new_watcher() -> VsCodeExtensionWatcher {
    VsCodeExtensionWatcher::new(CONFIG)
}
```

Then add to `mod.rs` and register with `registry.register(Box::new(my_extension::new_watcher()))`.

#### Option 2: CLI Tool or Custom Format

For tools with unique session formats, implement the `Watcher` trait:

```rust
// src/capture/watchers/newtool.rs
pub struct NewToolWatcher;

impl Watcher for NewToolWatcher {
    fn info(&self) -> WatcherInfo { ... }
    fn is_available(&self) -> bool { ... }
    fn find_sources(&self) -> Result<Vec<PathBuf>> { ... }
    fn parse_source(&self, path: &Path) -> Result<Vec<(Session, Vec<Message>)>> { ... }
    fn watch_paths(&self) -> Vec<PathBuf> { ... }
}
```

Use helpers from `common.rs`: `parse_role()`, `parse_timestamp_millis()`, `parse_uuid_or_generate()`.

#### Final Steps

1. Add the module to `src/capture/watchers/mod.rs`
2. Register it in `default_registry()`
3. Add tests for your parser
4. Update README.md to list the new tool

## Reporting Issues

When reporting issues, please include:

- Lore version (`lore --version`)
- Operating system and version
- Steps to reproduce the issue
- Expected vs actual behavior
- Relevant error messages

## Pull Request Process

1. Update documentation if needed
2. Add tests for new functionality
3. Ensure CI passes
4. Request review from maintainers
5. Address review feedback
6. Squash commits if requested

## AI-Assisted Contributions

Contributions developed with AI assistance are welcome. All contributions, regardless of how they were created, must meet the same quality standards and pass the same review process. The contributor submitting the PR is responsible for understanding and standing behind the code.

## Questions?

Open an issue for questions or discussions about contributing.

# Contributing to Lore

Thank you for your interest in contributing to Lore! This document provides guidelines and instructions for contributing.

## Getting Started

### Prerequisites

- Rust 1.70 or later
- Git
- SQLite development libraries (usually included with Rust's rusqlite)

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
- Write integration tests for CLI commands
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

To add support for a new AI coding tool:

1. Create a new file in `src/capture/watchers/`:
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

2. Add the module to `src/capture/watchers/mod.rs`

3. Register it in `WatcherRegistry::default_registry()`

4. Add tests for your parser

5. Update README.md to list the new tool

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

## Questions?

Open an issue for questions or discussions about contributing.

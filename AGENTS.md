# AGENTS.md - Kronk Development Guide

This file documents build commands, code style, and conventions for the Kronk project.

## Build & Testing

### Workspace Commands

```bash
# Build all crates
cargo build --workspace

# Release build
cargo build --release --workspace

# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test --package kronk-core

# Run a single test
cargo test --package kronk-core test_function_name

# Run a single test with full output
cargo test --package kronk-core test_function_name -- --nocapture

# Run tests with filtering
cargo test --package kronk-core -- backends::registry::tests::test_add

# Check formatting, clippy, and tests
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

### Makefile

```bash
make build        # Release build
make install      # Install CLI
make test         # Run all tests
make check        # fmt + clippy + test
make clippy       # Lint with -D warnings
make fmt          # Format all code
```

## Code Style

### Imports

- Group standard library imports first (`std::...`)
- Then external crates (`anyhow::...`, `tokio::...`)
- Then local module imports (`crate::...`)
- Use `use` for single imports, `use crate::...::*` for re-exports
- No unused imports
- Prefer `use anyhow::{anyhow, Context, Result}` over `use anyhow::Result` when using multiple items

### Formatting

- `cargo fmt --all` for formatting
- 4-space indentation
- No trailing whitespace
- Blank line between logical blocks
- Max line length: 100 chars (wrap naturally)

### Types

- Prefer `Result<T, E>` over `Option<T>` for fallible operations
- Use `anyhow::Result` (alias: `Result`) for error handling
- Use `anyhow::Context` for adding context to errors
- Structs derive `Debug`, `Clone`, `Serialize`, `Deserialize` when appropriate
- Use `#[derive(Default)]` for structs with sensible defaults

### Naming Conventions

- `snake_case` for functions, variables, modules
- `PascalCase` for types, structs, enums
- `UPPER_SNAKE_CASE` for constants
- Prefix test functions with `test_`
- Prefix private functions with `_` (e.g., `_hf_api()`)

### Error Handling

- Return `Result<T, E>` instead of `unwrap()` or `expect()` in public APIs
- Use `.with_context()` to add context to errors
- Use `anyhow::bail!` for early returns with errors
- Avoid `unreachable!()` - return errors for edge cases instead
- Chain errors with `?` operator where appropriate

### Documentation

- Add doc comments to public functions and structs
- Use `///` for single-line docs, `////` for multi-line
- Include `///` before `#[test]` for test documentation
- Document parameters and return values

## Testing

### Test Organization

- Tests in `#[cfg(test)]` modules at bottom of source files
- Group related tests with `mod tests { ... }`
- Use `#[tokio::test]` for async tests

### Test Patterns

```rust
#[test]
fn test_function_name() {
    // Arrange
    let input = "test input";
    
    // Act
    let result = my_function(input);
    
    // Assert
    assert_eq!(result, expected);
}

#[tokio::test]
async fn test_async_function() {
    let result = my_async_function().await;
    assert!(result.is_ok());
}

#[test]
#[serial]
fn test_concurrent_access() {
    // Use serial attribute for tests with shared state
}
```

### Test Helpers

- Create helper functions in `tests/` module
- Use `tempfile::tempdir()` for temporary directories
- Use `assert_matches!` for pattern matching on Results
- Use `assert!(condition, "custom message")` for custom error messages

## Project Structure

```
kronk/
├── crates/
│   ├── kronk-core/      # Core library (types, models, logic)
│   ├── kronk-cli/       # CLI application
│   └── kronk-mock/      # Mock utilities for testing
├── config/              # Configuration templates
├── docs/                # Documentation
├── installer/           # Windows installer scripts
└── target/              # Build artifacts (ignored)
```

## Conventions

### TDD Approach

1. Write failing test
2. Verify it fails
3. Implement minimal code
4. Verify test passes
5. Refactor if needed
6. Commit frequently

### Code Review

- Follow DRY principle
- No premature optimization (YAGNI)
- Prefer composition over inheritance
- Keep functions small and focused
- Add tests for edge cases

### Git Workflow

- Feature branches from `main`
- Descriptive commit messages
- `feat:`, `fix:`, `chore:`, `docs:` prefixes
- Push to remote before merging

## No External Rules

This project does not use Cursor rules (.cursor/) or Copilot instructions (.github/copilot-instructions.md).
# Contributing to Calimero Core with Cursor

This guide helps contributors get the best experience when using [Cursor](https://cursor.sh) to work on the Calimero Core project.

## Getting Started

### Prerequisites

1. **Install Rust toolchain:**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup default stable
   rustup component add rustfmt clippy
   ```

2. **Clone and open in Cursor:**
   ```bash
   git clone https://github.com/calimero-network/core.git
   cd core
   cursor .  # or open via Cursor GUI
   ```

3. **Verify toolchain:**
   ```bash
   cargo --version
   rustfmt --version
   ```

## Project Structure

### Key Entry Points

- **`crates/merod/`** - Main daemon binary (node runtime)
- **`crates/meroctl/`** - CLI tool for interacting with nodes
- **`crates/server/`** - HTTP/WebSocket server, admin API
- **`crates/runtime/`** - WASM execution engine (security-critical)
- **`crates/auth/`** - Authentication and JWT handling
- **`crates/context/`** - Context management and execution
- **`crates/network/`** - libp2p networking layer
- **`crates/storage/`** - CRDT storage layer
- **`crates/sdk/`** - Developer SDK for building apps

### Important Files

- `Cargo.toml` - Workspace configuration
- `rust-toolchain.toml` - Required Rust version
- `rustfmt.toml` - Formatting configuration
- `deny.toml` - Dependency auditing rules

## Cursor Best Practices

### Using Cursor Rules

When Cursor doesn't have a `.cursorrules` file, you can guide it by:

1. **Starting prompts with context:**
   > "In this Rust workspace for a distributed application platform..."

2. **Referencing key conventions:**
   - Use `eyre::Result` for error handling in most crates
   - Follow the existing module structure
   - Add tracing logs at appropriate levels

### Composer vs Agent

- **Use Composer** for quick edits, refactors, and understanding code
- **Use Agent** for multi-step tasks like implementing a bounty across multiple files
- **Use Terminal** for running tests and cargo commands

### Effective Prompts

Good prompts for this codebase:

```
"Add input validation to the InstallApplicationRequest handler in crates/server"

"Refactor the VMLogic memory functions to return proper errors instead of panicking"

"Add tests for the JWT token rotation logic in crates/auth"
```

## Development Workflow

### Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p calimero-runtime

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_storage_write_read
```

### Formatting and Linting

```bash
# Format code (required before commits)
cargo fmt

# Check formatting without changing files
cargo fmt --check

# Run clippy lints
cargo clippy --all-targets --all-features

# Fix clippy suggestions automatically
cargo clippy --fix --allow-dirty
```

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Build specific binary
cargo build -p merod
cargo build -p meroctl
```

## Working on Bounties

### Finding a Bounty

1. Review `bounties.json` in the repository root
2. Look for bounties matching your expertise (security, concurrency, tests, etc.)
3. Check the `pathHint` to understand which files are involved

### Bounty Workflow

1. **Understand the issue:**
   ```
   Read the bounty description carefully
   Open the pathHint file in Cursor
   Use Cursor to explain the relevant code
   ```

2. **Make minimal changes:**
   - Focus on the specific issue described
   - Avoid refactoring unrelated code
   - Keep commits atomic and focused

3. **Validate your changes:**
   ```bash
   cargo fmt
   cargo clippy
   cargo test -p <affected-crate>
   ```

4. **Commit with conventional format:**
   ```bash
   git add .
   git commit -m "fix(runtime): add bounds checking to WASM memory access"
   ```

### Commit Message Format

Use [Conventional Commits](https://www.conventionalcommits.org/):

- `fix:` - Bug fixes
- `feat:` - New features
- `docs:` - Documentation changes
- `refactor:` - Code refactoring
- `test:` - Adding or updating tests
- `security:` - Security-related fixes

Examples:
```
fix(auth): handle RwLock poisoning in token manager
feat(server): add request ID to error responses
docs(runtime): document unsafe memory access patterns
test(storage): add property tests for CRDT merge
security(server): restrict CORS to configured origins
```

## Code Conventions

### Error Handling

```rust
// Preferred: typed errors with eyre
use eyre::{Result, WrapErr};

fn do_something() -> Result<()> {
    operation().wrap_err("failed to perform operation")?;
    Ok(())
}

// For public APIs: custom error types
#[derive(Debug, thiserror::Error)]
pub enum MyError {
    #[error("operation failed: {0}")]
    OperationFailed(String),
}
```

### Logging

```rust
use tracing::{debug, error, info, warn};

// Use appropriate levels
error!(%context_id, %err, "Critical failure");
warn!(%context_id, "Recoverable issue");
info!(%context_id, "Important event");
debug!(%context_id, "Debug details");
```

### Async Code

```rust
// Avoid blocking in async contexts
// Bad:
let result = std::thread::sleep(duration);

// Good:
let result = tokio::time::sleep(duration).await;

// For blocking operations in async:
let result = tokio::task::spawn_blocking(|| {
    expensive_computation()
}).await?;
```

## Security Considerations

When working on security-related bounties:

1. **Never trust user input** - Validate all external data
2. **Check bounds** before memory access in WASM runtime
3. **Use typed errors** - Don't leak internal details in error messages
4. **Add timeouts** - All network/IO operations should have limits
5. **Log security events** - But never log secrets or tokens

## Getting Help

- Check existing code for patterns
- Use Cursor to explain unfamiliar code sections
- Look at test files for usage examples
- Review PR history for similar changes

## CI/CD

Before opening a PR, ensure:

1. `cargo fmt --check` passes
2. `cargo clippy` has no warnings
3. `cargo test` passes
4. Commit messages follow conventional format

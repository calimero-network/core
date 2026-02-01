# Cursor Contributor Guide for Calimero Core

This guide helps contributors get the most out of Cursor AI when contributing to the Calimero Core repository.

## Getting Started

### Prerequisites

1. **Install Rust toolchain**:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source ~/.cargo/env
   ```

2. **Install required components**:
   ```bash
   rustup component add rustfmt clippy
   ```

3. **Clone the repository**:
   ```bash
   git clone https://github.com/calimero-network/core.git
   cd core
   ```

4. **Open in Cursor**:
   ```bash
   cursor .
   ```

### Environment Setup

The repository uses specific Rust versions defined in `rust-toolchain.toml`. Cursor will automatically detect this.

Key environment variables for testing:
- `RUST_LOG=debug` - Enable debug logging
- `ENABLE_WASMER_PROFILING=true` - Enable WASM profiling (optional)

## Repository Structure

Understanding the codebase organization:

```
core/
├── crates/           # Main Rust crates (the core of the project)
│   ├── auth/         # Authentication service (mero-auth)
│   ├── context/      # Context management and execution
│   ├── crypto/       # Cryptographic primitives
│   ├── merod/        # Main daemon binary
│   ├── meroctl/      # CLI tool
│   ├── network/      # P2P networking (libp2p)
│   ├── node/         # Node orchestration and sync
│   ├── primitives/   # Core data types
│   ├── runtime/      # WASM runtime (wasmer)
│   ├── sdk/          # Developer SDK and macros
│   ├── server/       # HTTP/WS server
│   ├── storage/      # CRDT storage layer
│   └── store/        # Database abstraction (RocksDB)
├── apps/             # Example applications
├── tools/            # Development tools
└── scripts/          # Build and deployment scripts
```

### Key Entry Points

- **Main daemon**: `crates/merod/src/main.rs`
- **CLI tool**: `crates/meroctl/src/main.rs`
- **Auth service**: `crates/auth/src/main.rs`
- **Relayer**: `crates/relayer/src/main.rs`

### Critical Modules (require careful review)

- `crates/runtime/src/logic.rs` - WASM execution with unsafe code
- `crates/auth/src/auth/token/jwt.rs` - JWT handling
- `crates/crypto/src/lib.rs` - Cryptographic operations
- `crates/node/src/sync/` - State synchronization

## Working with Cursor

### Cursor Best Practices

1. **Use Composer for large changes**: When working on bounties that touch multiple files, use Cursor's Composer feature to maintain context across files.

2. **Use Agent for exploration**: When investigating unfamiliar code, use Agent mode to explore dependencies and call chains.

3. **Terminal integration**: Run tests directly in Cursor's integrated terminal to see results alongside code.

4. **Inline chat for quick fixes**: For small changes, use inline chat (Cmd/Ctrl+K) to make targeted edits.

### Recommended Cursor Settings

Add these to your Cursor settings for this project:

```json
{
  "rust-analyzer.cargo.features": "all",
  "rust-analyzer.checkOnSave.command": "clippy",
  "editor.formatOnSave": true,
  "[rust]": {
    "editor.defaultFormatter": "rust-lang.rust-analyzer"
  }
}
```

## Development Workflow

### Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p calimero-runtime

# Run tests with output
cargo test -- --nocapture

# Run a specific test
cargo test test_name
```

### Formatting and Linting

**Always run before committing:**

```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --all-targets --all-features -- -D warnings

# Check for common issues
cargo check
```

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Build specific binary
cargo build -p merod
```

## Working on Bounties

### Finding Bounties

1. Check `bounties.json` in the repository root
2. Each bounty has:
   - `title`: What needs to be done
   - `description`: Details and file locations
   - `pathHint`: Where to start looking
   - `estimatedMinutes`: Expected effort
   - `category`: Type of work (security, bug, design-flaw, etc.)
   - `severity`: Priority (critical, high, medium, low)

### Bounty Workflow

1. **Pick a bounty**: Start with the `pathHint` file
2. **Understand context**: Use Cursor to explore related code
3. **Make minimal changes**: Focus on the specific issue
4. **Add tests**: Include tests for new functionality
5. **Run checks**: `cargo fmt && cargo clippy && cargo test`
6. **Commit with conventional format**

### Example: Working on a Security Bounty

```bash
# 1. Find the file mentioned in pathHint
cursor crates/auth/src/config.rs

# 2. Read related code to understand context
# Use Cursor's "Go to Definition" and "Find References"

# 3. Make changes

# 4. Test your changes
cargo test -p mero-auth

# 5. Format and lint
cargo fmt && cargo clippy

# 6. Commit
git add -A
git commit -m "fix(auth): remove unsafe CSP defaults"
```

## Commit Guidelines

### Conventional Commits

All commits must follow the conventional commit format:

```
<type>(<scope>): <description>
```

**Types:**
- `fix`: Bug fixes
- `feat`: New features
- `docs`: Documentation changes
- `refactor`: Code refactoring
- `test`: Adding tests
- `perf`: Performance improvements
- `security`: Security fixes

**Examples:**
```
fix(auth): remove unsafe-eval from default CSP
feat(runtime): add memory bounds checking for WASM
docs(sdk): add examples for event handling
refactor(storage): consolidate CRDT implementations
test(sync): add property tests for delta merge
security(crypto): implement proper key zeroization
```

### PR Guidelines

1. **Title**: Use conventional commit format
2. **Description**: Explain what changed and why
3. **Test plan**: Describe how you tested the changes
4. **Documentation**: Note any doc updates needed

## Testing Requirements

### For Bug Fixes

- Add a test that fails without the fix
- Verify the test passes with the fix

### For New Features

- Add unit tests for new functions
- Add integration tests for new APIs
- Document public interfaces

### For Security Changes

- Add negative tests (invalid inputs should fail)
- Test edge cases and boundary conditions
- Consider adding fuzzing targets

## Common Patterns

### Error Handling

Use `eyre::Result` for fallible operations:

```rust
use eyre::{Result, bail, WrapErr};

fn do_something() -> Result<()> {
    something_fallible()
        .wrap_err("context about what we were doing")?;
    Ok(())
}
```

### Logging

Use `tracing` for logging:

```rust
use tracing::{debug, info, warn, error};

info!(%context_id, "Processing request");
debug!(?complex_data, "Debug details");
error!(%err, "Operation failed");
```

### Async Code

Avoid blocking in async contexts:

```rust
// Good: Use spawn_blocking for CPU-heavy work
let result = tokio::task::spawn_blocking(|| expensive_computation())
    .await?;

// Bad: Blocking directly
let result = expensive_computation(); // Blocks executor!
```

## Getting Help

- **Documentation**: Check `docs/` and README files
- **Style guide**: Read `STYLE.md`
- **Existing code**: Look at similar implementations
- **Issues**: Search existing GitHub issues
- **Discussions**: Ask in GitHub Discussions

## Security Considerations

When working on security-sensitive code:

1. Never log secrets, tokens, or private keys
2. Validate all inputs at trust boundaries
3. Use `zeroize` for sensitive data
4. Add tests for malicious inputs
5. Document security assumptions

## Quick Reference

```bash
# Format
cargo fmt

# Lint
cargo clippy

# Test
cargo test

# Build
cargo build

# Check
cargo check

# Doc
cargo doc --open
```

Happy contributing!

# Cursor Contributing Guide for Calimero

This guide helps contributors get the most out of Cursor when working on the Calimero codebase.

## Prerequisites

### Rust Toolchain

```bash
# Install Rust via rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add required components
rustup component add rustfmt clippy

# For WASM development (optional)
rustup target add wasm32-unknown-unknown
```

### Clone and Open in Cursor

```bash
git clone https://github.com/calimero-network/core.git
cd core
cursor .
```

## Project Structure

Calimero is a workspace with multiple crates. Understanding the structure helps Cursor provide better suggestions:

| Directory | Purpose |
|-----------|---------|
| `crates/merod/` | Node daemon (main entry point) |
| `crates/meroctl/` | CLI tool for node interaction |
| `crates/server/` | HTTP/WS/SSE API server |
| `crates/runtime/` | WASM execution runtime |
| `crates/context/` | Context (application state) management |
| `crates/storage/` | CRDT-based storage layer |
| `crates/store/` | Key-value storage abstraction |
| `crates/network/` | libp2p networking |
| `crates/node/` | Node orchestration and sync |
| `crates/auth/` | Authentication service |
| `crates/sdk/` | SDK for WASM applications |
| `crates/primitives/` | Shared types and constants |
| `apps/` | Example WASM applications |
| `tools/` | Development tools |

## Development Workflow

### Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p calimero-runtime

# Run with verbose output
cargo test -- --nocapture
```

### Formatting and Linting

```bash
# Format code (required before commits)
cargo fmt

# Run clippy lints
cargo clippy --all-targets --all-features

# Fix clippy warnings automatically
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
```

## Using Cursor Effectively

### Composer vs Agent

- **Composer** (Cmd/Ctrl + K): Best for focused edits within a single file
  - Refactoring functions
  - Adding error handling
  - Implementing traits

- **Agent** (Cmd/Ctrl + Shift + I): Best for multi-file changes
  - Adding new features across crates
  - Fixing bugs that span multiple modules
  - Creating tests alongside implementations

### Context Tips

When asking Cursor to help with Calimero:

1. **Reference specific crates**: "In `crates/runtime/src/lib.rs`, help me..."
2. **Mention the domain**: "For WASM execution...", "In the CRDT sync protocol..."
3. **Include error messages**: Copy full compiler errors for accurate fixes

### Useful Queries

- "Explain the actor pattern used in this crate"
- "Show me how errors propagate from runtime to server"
- "What are the safety invariants for this unsafe block?"
- "Add tests for this function covering edge cases"

## Working on Bounties

### Finding Bounties

Check `bounties.json` in the repository root for available tasks:

```bash
# View bounties by severity
jq '.bounties | sort_by(.severity) | reverse' bounties.json
```

### Bounty Workflow

1. **Pick a bounty**: Choose one matching your skill level
2. **Understand the context**: Read the `pathHint` file and surrounding code
3. **Create a branch**: `git checkout -b fix/bounty-title`
4. **Make minimal changes**: Focus on the specific issue
5. **Run tests**: `cargo test -p <affected-crate>`
6. **Format code**: `cargo fmt`
7. **Run clippy**: `cargo clippy`
8. **Commit with conventional format**: `fix(runtime): add timeout for WASM execution`

### Conventional Commits

Use conventional commit format for PR titles and commits:

| Prefix | Use Case |
|--------|----------|
| `fix:` | Bug fixes |
| `feat:` | New features |
| `docs:` | Documentation |
| `refactor:` | Code restructuring |
| `test:` | Adding tests |
| `perf:` | Performance improvements |
| `security:` | Security fixes |

Examples:
- `fix(server): add rate limiting to prevent DoS`
- `feat(runtime): implement execution timeout`
- `docs(sdk): add macro usage examples`

## Code Patterns

### Error Handling

The codebase uses typed errors. Follow existing patterns:

```rust
// Good: Typed error
return Err(ExecuteError::ContextNotFound);

// Avoid: String errors
bail!("context not found");
```

### Logging

Use structured logging with tracing:

```rust
use tracing::{debug, error, info, warn};

info!(%context_id, method, "Executing method");
error!(%context_id, %err, "Failed to execute");
```

### Async Patterns

The codebase uses both Actix actors and async/await:

```rust
// Actor messages
impl Handler<ExecuteRequest> for ContextManager {
    type Result = ActorResponse<Self, Result>;
    // ...
}

// Async functions
async fn handle_request(req: Request) -> Result<Response> {
    // ...
}
```

### Safety Comments

All unsafe blocks should have SAFETY comments:

```rust
// SAFETY: The pointer is valid for the lifetime of the storage
// and we have exclusive access during this host function call.
unsafe { /* ... */ }
```

## Testing Guidelines

### Unit Tests

Place in the same file or `tests/` submodule:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_specific_behavior() {
        // Arrange
        // Act
        // Assert
    }
}
```

### Integration Tests

Place in `tests/` directory of each crate.

### Property-Based Tests

For complex logic (CRDT merging, serialization):

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn merge_is_commutative(a: Data, b: Data) {
        assert_eq!(merge(a, b), merge(b, a));
    }
}
```

## Common Issues

### Compilation Takes Too Long

Use cargo's incremental compilation and limit parallel jobs:

```bash
CARGO_BUILD_JOBS=4 cargo build
```

### Clippy Warnings

The CI enforces clippy warnings. Fix or use expect with reason:

```rust
#[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
fn complex_function() { /* ... */ }
```

### WASM Build Issues

Ensure you have the WASM target:

```bash
rustup target add wasm32-unknown-unknown
```

## Getting Help

- Check existing issues and PRs for similar problems
- Read crate-level documentation in `src/lib.rs` files
- Review the `README.md` files in each crate
- Ask Cursor to explain unfamiliar patterns

## Security Considerations

When working on security-sensitive code:

1. **Never commit secrets**: Check for API keys, private keys
2. **Validate all inputs**: Especially from network or WASM guests
3. **Use constant-time comparisons**: For cryptographic operations
4. **Document unsafe blocks**: Explain invariants and safety
5. **Add tests for edge cases**: Malformed inputs, boundary conditions

## Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Async Rust](https://rust-lang.github.io/async-book/)
- [Actix Actors](https://actix.rs/docs/actix/)
- [libp2p](https://docs.libp2p.io/)
- [Wasmer](https://docs.wasmer.io/)

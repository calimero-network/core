# Contributing to Calimero Core with Cursor

This guide helps contributors get the best experience when working on Calimero Core using Cursor IDE.

## Getting Started

### Prerequisites

1. **Install Rust toolchain**:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup component add rustfmt clippy
   ```

2. **Clone and open in Cursor**:
   ```bash
   git clone https://github.com/calimero-network/core.git
   cd core
   cursor .
   ```

3. **Verify toolchain** (pinned in `rust-toolchain.toml`):
   ```bash
   rustc --version  # Should show 1.88.0
   cargo fmt --version
   ```

### Repository Structure

Key crates to understand:

| Crate | Purpose | Entry Point |
|-------|---------|-------------|
| `merod` | Node daemon CLI | `crates/merod/src/main.rs` |
| `meroctl` | Client CLI tool | `crates/meroctl/src/main.rs` |
| `runtime` | WASM execution | `crates/runtime/src/lib.rs` |
| `server` | HTTP/WS APIs | `crates/server/src/lib.rs` |
| `auth` | Authentication | `crates/auth/src/main.rs` |
| `network` | P2P networking | `crates/network/src/lib.rs` |
| `storage` | CRDT storage | `crates/storage/src/lib.rs` |
| `context` | Context management | `crates/context/src/lib.rs` |
| `node` | Node orchestration | `crates/node/src/lib.rs` |

## Using Cursor Effectively

### Cursor Rules

The repository includes `.cursorrules` with project conventions. Cursor will automatically apply these when generating code. Key points:

- Use StdExternalCrate import ordering
- Module-level import granularity (no nested imports across modules)
- Use `eyre::Result` for error handling
- Avoid `.unwrap()` and `.expect()` in production code
- Follow conventional commit format

### Composer vs Agent

- **Composer** (Cmd/Ctrl+K): Best for focused edits within a single file
  - Refactoring a function
  - Adding error handling
  - Implementing a trait

- **Agent** (Cmd/Ctrl+I): Best for multi-file changes
  - Implementing a feature across crates
  - Adding tests for existing code
  - Refactoring patterns across the codebase

### Terminal Integration

Run commands directly in Cursor's terminal:

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p calimero-runtime

# Check formatting
cargo fmt --check

# Run clippy (with warnings allowed, as per CI)
cargo clippy -- -A warnings

# Check licenses
cargo deny check licenses sources
```

## Working on Bounties

### Finding a Bounty

1. Check `bounties.json` in the repository root
2. Each bounty includes:
   - `title`: Brief description
   - `description`: What to fix and where
   - `pathHint`: Starting file/directory
   - `estimatedMinutes`: Expected effort
   - `severity`: Priority level

### Workflow

1. **Understand the issue**: Read the bounty description and locate the `pathHint`

2. **Explore context**: Use Cursor to explore related code:
   - `Cmd+Click` to jump to definitions
   - Use Agent to ask "Explain how X works"

3. **Make minimal changes**: Follow the principle of least change:
   - Fix only what's described
   - Don't refactor unrelated code
   - Add tests for your fix

4. **Verify locally**:
   ```bash
   # Format code
   cargo fmt

   # Check for issues
   cargo clippy -- -A warnings

   # Run tests
   cargo test

   # If you modified storage/runtime, run integration tests
   cargo test -p calimero-node --test '*'
   ```

5. **Commit with conventional format**:
   ```bash
   # Types: feat, fix, docs, refactor, test, chore
   git add .
   git commit -m "fix(auth): add rate limiting to token endpoints"
   ```

### Example: Working on a Security Bounty

For the bounty "Fix CSP defaults allowing 'unsafe-inline'":

1. Open `crates/auth/src/config.rs`
2. Find `default_csp_script_src()` function
3. Use Agent: "How can I replace 'unsafe-inline' with nonces in this CSP configuration?"
4. Implement the change
5. Update tests in the same crate
6. Run `cargo test -p mero-auth`

## Code Style Quick Reference

### Imports

```rust
// Standard library first
use std::collections::HashMap;
use std::sync::Arc;

// External crates
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// Local crate
use crate::types::Config;
```

### Error Handling

```rust
// Preferred: propagate with context
fn process() -> eyre::Result<()> {
    let data = load_data()
        .wrap_err("failed to load data")?;
    Ok(())
}

// Avoid in production
fn bad() {
    let data = load_data().unwrap(); // Don't do this
}
```

### Early Returns

```rust
// Preferred: short-circuit
fn validate(input: &str) -> Result<(), Error> {
    if input.is_empty() {
        return Err(Error::Empty);
    }
    // Continue with validated input
    Ok(())
}
```

## CI Checks

Before submitting a PR, ensure all CI checks pass:

| Check | Command | Notes |
|-------|---------|-------|
| Format | `cargo fmt --check` | Auto-fix with `cargo fmt` |
| Clippy | `cargo clippy -- -A warnings` | Warnings allowed |
| Tests | `cargo test` | All tests must pass |
| Licenses | `cargo deny check licenses sources` | Check dependency licenses |

## Getting Help

- **Documentation**: Check `README.mdx` and crate-level `README.md` files
- **Architecture**: See `.cursorrules` for conventions
- **Issues**: Search existing GitHub issues before asking

## Tips for Cursor

1. **Use @file references**: When asking about code, reference files with `@crates/auth/src/config.rs`

2. **Provide context**: Include error messages and relevant code snippets

3. **Iterate**: Start with small changes, verify they work, then expand

4. **Trust the rules**: The `.cursorrules` file contains project-specific guidance that Cursor will follow

5. **Test incrementally**: Run tests after each significant change to catch issues early

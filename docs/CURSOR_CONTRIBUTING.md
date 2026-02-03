# Cursor Contributor Guide for Calimero Core

This guide helps contributors get the best experience when using [Cursor](https://cursor.sh) to contribute to Calimero Core.

## Quick Start

### 1. Clone and Open

```bash
git clone https://github.com/calimero-network/core.git
cd core
cursor .
```

### 2. Environment Setup

Ensure you have the required toolchain:

```bash
# Install Rust via rustup (version is pinned in rust-toolchain.toml)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# The pinned version (1.88.0) will be automatically installed
# Verify installation
rustc --version
cargo --version

# Install rustfmt for code formatting
rustup component add rustfmt

# For WASM development (optional)
rustup target add wasm32-unknown-unknown
```

## Cursor Configuration

### Using Cursor Rules

This repository includes a `.cursorrules` file that provides AI assistants with project-specific context. The rules cover:

- **Import organization**: StdExternalCrate pattern
- **Module structure**: No `mod.rs` pattern
- **Error handling**: Use `eyre` crate
- **Code style**: Early returns, `let..else` patterns
- **Testing**: `#[cfg(test)]` module placement

When working with Cursor Agent or Composer, these rules are automatically applied to suggestions.

### Recommended Settings

In Cursor settings, enable:
- **Codebase indexing**: For accurate autocomplete across all crates
- **Terminal integration**: For running cargo commands inline

## Repository Structure

Key entry points for understanding the codebase:

| Crate | Purpose | Entry Point |
|-------|---------|-------------|
| `merod` | Node daemon | `crates/merod/src/main.rs` |
| `meroctl` | CLI tool | `crates/meroctl/src/main.rs` |
| `calimero-runtime` | WASM execution | `crates/runtime/src/lib.rs` |
| `calimero-node` | Node orchestration | `crates/node/src/lib.rs` |
| `calimero-storage` | CRDT collections | `crates/storage/src/lib.rs` |

Each folder in `crates/` is a separate crate - treat them as independent projects.

## Development Workflow

### Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p calimero-runtime
cargo test -p calimero-storage

# Run tests with output
cargo test -- --nocapture

# Run a specific test
cargo test -p calimero-dag test_dag_out_of_order -- --nocapture
```

### Formatting and Linting

Always run before committing:

```bash
# Format code
cargo fmt

# Check formatting without modifying
cargo fmt --check

# Run clippy
cargo clippy -- -A warnings

# Check licenses
cargo deny check licenses sources
```

### Building WASM Apps

```bash
# Build example apps
rustup target add wasm32-unknown-unknown
cargo build -p kv-store --target wasm32-unknown-unknown --release

# Build all apps
./scripts/build-all-apps.sh
```

### Running Locally

```bash
# Initialize and run a node
cargo build -p merod --release
./target/release/merod --node node1 init --server-port 2428 --swarm-port 2528
./target/release/merod --node node1 run

# With debug logging
RUST_LOG=debug ./target/release/merod --node node1 run
```

## Working on Bounties

### Finding a Bounty

1. Check `bounties.json` in the repository root
2. Pick a bounty matching your skill level (check `estimatedMinutes` and `severity`)
3. Use `pathHint` to navigate to the relevant code

### Using Cursor Agent for Bounties

When working on a bounty with Cursor Agent:

1. Open the file indicated in `pathHint`
2. Press `Cmd/Ctrl + L` to open Composer
3. Paste the bounty description and ask for implementation guidance
4. Review suggestions against the `.cursorrules` style guide

**Example prompt:**
```
I'm working on this bounty: "Add rate limiting to authentication endpoints"

The path hint is crates/auth/src/server.rs. 

Can you help me implement per-IP rate limiting using tower-governor, 
following the project's error handling patterns with eyre?
```

### Before Committing

1. Run the test suite: `cargo test`
2. Format your code: `cargo fmt`
3. Check for lint issues: `cargo clippy`
4. Verify CI will pass: `cargo deny check licenses sources`

## Commit Conventions

Use conventional commits for all PRs:

```
<type>(<scope>): <short summary>
```

**Types:**
- `fix`: Bug fixes
- `feat`: New features
- `docs`: Documentation changes
- `refactor`: Code refactoring
- `test`: Adding/fixing tests
- `chore`: Build/tooling changes

**Examples:**
```
fix(runtime): validate WASM memory bounds before access
feat(auth): add per-IP rate limiting to token endpoints
docs(storage): document CRDT merge invariants
test(dag): add property-based tests for delta merging
```

## Debugging Tips

### Using merodb for Database Inspection

```bash
# Inspect database schema
cargo run -p merodb -- --db-path ~/.calimero/node1/data --schema

# Export state
cargo run -p merodb -- --db-path ~/.calimero/node1/data --export --all \
  --wasm-file ./my_app.wasm --output export.json

# Validate integrity
cargo run -p merodb -- --db-path ~/.calimero/node1/data --validate
```

### Enabling Debug Logging

```bash
# Per-crate logging
RUST_LOG=calimero_runtime=debug,calimero_storage=trace merod --node node1 run

# Network debugging
RUST_LOG=calimero_network=debug,libp2p=debug merod --node node1 run
```

### Using meroctl for Inspection

```bash
# List contexts
meroctl --node node1 context ls

# Call a method (for testing)
meroctl --node node1 call <context_id> --method get_value --args '{"key": "test"}'

# List peers
meroctl --node node1 peers ls
```

## Common Issues

### Tests Failing with "context not found"

The test might need a running node or mock storage. Check if the test uses `#[tokio::test]` and initializes storage properly.

### Clippy Warnings

Some crates have specific clippy configurations. Check the crate's `lib.rs` for `#![allow(...)]` or `#![deny(...)]` directives.

### Import Organization Errors

Follow the StdExternalCrate pattern from `.cursorrules`:
1. Standard library
2. External crates
3. Local crate (`crate::`, `super::`)
4. Local modules

### WASM Compilation Issues

Ensure you're using the correct target:
```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release -p <app-name>
```

## Getting Help

- Check existing issues on GitHub
- Review `.cursorrules` for style guidance
- Ask in Cursor Agent with context from relevant files
- Reference the architecture section in `.cursorrules` for understanding data flow

## Additional Resources

- [README.mdx](../README.mdx) - Project overview
- [CONTRIBUTING.md](../CONTRIBUTING.md) - General contribution guidelines
- [STYLE.md](../STYLE.md) - Code style guide
- [.cursorrules](../.cursorrules) - AI-specific project rules

# Cursor Contributor Guide for Calimero Core

This guide helps contributors get the most out of Cursor when working on the Calimero Core codebase.

## Opening the Repository in Cursor

### Prerequisites

1. **Install Rust toolchain**:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Ensure correct Rust version** (pinned in `rust-toolchain.toml`):
   ```bash
   rustup show  # Should show 1.88.0
   ```

3. **Install required components**:
   ```bash
   rustup component add rustfmt clippy
   ```

### Clone and Open

```bash
git clone https://github.com/calimero-network/core.git
cd core
cursor .  # Open in Cursor
```

## Cursor Configuration

### Using .cursorrules

The repository includes a `.cursorrules` file that provides AI guidance for:
- Import organization (StdExternalCrate pattern)
- Module structure (no `mod.rs` pattern)
- Error handling with `eyre`
- Naming conventions
- Commit message format

Cursor will automatically use these rules when generating code suggestions.

### Recommended Settings

For best results, ensure Cursor has access to:
- Full workspace indexing (for accurate code navigation)
- Terminal integration (for running tests)

## Repository Structure

### Key Entry Points

| Path | Description |
|------|-------------|
| `crates/merod/` | Node daemon - main entry point for running nodes |
| `crates/meroctl/` | CLI tool for node management |
| `crates/auth/` | Authentication service |
| `crates/server/` | HTTP/WebSocket server |
| `crates/runtime/` | WebAssembly runtime (security-critical) |
| `crates/network/` | P2P networking with libp2p |
| `crates/storage/` | CRDT-based storage system |
| `crates/context/` | Context management |
| `crates/sdk/` | SDK for application development |

### Crate Organization

- **`*-primitives`**: Shared types (e.g., `calimero-context-primitives`)
- **`*-config`**: Configuration types
- Each folder is conceptually a separate project

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

```bash
# Format code (required before commits)
cargo fmt

# Check formatting without modifying
cargo fmt --check

# Run clippy lints
cargo clippy -- -A warnings

# Full CI checks
cargo deny check licenses sources
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
2. Each bounty includes:
   - `title`: Brief description
   - `description`: What's wrong and what to do
   - `pathHint`: Where to start looking
   - `estimatedMinutes`: Expected time
   - `category`: Type of work
   - `severity`: Priority level

### Using Cursor Agent for Bounties

1. **Open the bounty file**:
   ```
   @bounties.json
   ```

2. **Ask Cursor to help**:
   - "Show me bounty #5 and the relevant code"
   - "Help me fix the CORS configuration issue in crates/server/src/lib.rs"

3. **Navigate to pathHint**:
   Use `Cmd+P` (macOS) or `Ctrl+P` (Windows/Linux) to open files

### Best Practices

1. **Make minimal changes**: Fix only what's described
2. **Add tests**: Especially for bug fixes
3. **Run tests locally** before committing
4. **Follow existing patterns** in the codebase

## Using Cursor Features

### Composer vs Agent

| Feature | Use Case |
|---------|----------|
| **Composer** | Quick edits, single-file changes, refactoring |
| **Agent** | Multi-file changes, exploring codebase, complex bounties |

### Effective Prompts

**Good prompts**:
- "Explain the error handling pattern in crates/runtime/src/errors.rs"
- "Add bounds checking to read_guest_memory_slice in crates/runtime/src/logic.rs"
- "Create a test for the edge case where max_registers is exceeded"

**Avoid**:
- Vague requests without file references
- Asking for changes across many files at once

### Terminal Integration

Use Cursor's terminal for:
- Running `cargo test`
- Checking `cargo clippy` output
- Building specific crates

## Commit Guidelines

### Format

```
<type>(<scope>): <short summary>
```

### Types

| Type | Description |
|------|-------------|
| `fix` | Bug fix |
| `feat` | New feature |
| `docs` | Documentation |
| `refactor` | Code restructuring |
| `test` | Adding/fixing tests |
| `perf` | Performance improvement |

### Examples

```bash
git commit -m "fix(runtime): add bounds checking to guest memory access"
git commit -m "test(storage): add edge case tests for VMLimits"
git commit -m "docs(sdk): improve macro error messages"
```

## Pull Request Checklist

Before submitting a PR:

- [ ] `cargo fmt` passes
- [ ] `cargo clippy -- -A warnings` passes
- [ ] `cargo test` passes
- [ ] Changes follow `.cursorrules` guidelines
- [ ] Commit messages follow conventional format
- [ ] PR title follows conventional format
- [ ] Description includes test plan

## Common Issues

### Rust Version Mismatch

If you see unexpected errors:
```bash
rustup override set 1.88.0
```

### Missing Dependencies

For RocksDB issues:
```bash
# Ubuntu/Debian
sudo apt install libclang-dev

# macOS
brew install llvm
```

### Test Failures

Some tests require specific setup:
```bash
# Run only unit tests (no integration)
cargo test --lib

# Skip network tests
cargo test -- --skip network
```

## Getting Help

- Check existing issues: https://github.com/calimero-network/core/issues
- Read the style guide: `STYLE.md`
- Review contributing guide: `CONTRIBUTING.md`
- Ask in PR comments for clarification

## Quick Reference

```bash
# Full workflow for a bounty fix
cargo fmt                    # Format
cargo clippy -- -A warnings  # Lint  
cargo test                   # Test
git add -p                   # Stage changes
git commit -m "fix(scope): description"
git push origin my-branch
```

# Calimero Client

This is the main Rust client library for Calimero Network APIs.

## Structure

- **`Cargo.toml`** - Rust package configuration
- **`python/`** - Python bindings and packaging (separate project)

## Python Bindings

The Python bindings are now organized in their own directory at `python/`. This keeps the Rust client code clean and separates concerns.

To work with the Python bindings:

```bash
cd python/
maturin build --features python
```

See `python/README.md` for complete Python package documentation.

## Rust Development

For Rust development:

```bash
cargo build
cargo test
cargo build --features python  # Build with Python bindings
```

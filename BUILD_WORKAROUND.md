# Build Script Workaround

## Problem

The `calimero-server` and `mero-auth` crates have build scripts that make network requests to download assets. On macOS, these build scripts fail with a `system-configuration` NULL object panic due to a known issue in the `reqwest` dependency.

## Solution

Set the `CALIMERO_WEBUI_SRC` environment variable to a local directory to bypass the network requests:

```bash
# Create dummy directory
mkdir -p /tmp/webui

# Run cargo commands with environment variable
export CALIMERO_WEBUI_SRC=/tmp/webui

cargo fmt --all --check
cargo clippy -p calimero-protocols -p calimero-sync -p calimero-node -p calimero-context --lib -- -A warnings
cargo test -p calimero-protocols -p calimero-sync -p calimero-node -p calimero-context --lib
```

## Automated Script

Use the provided CI check script:

```bash
./scripts/ci-check.sh
```

This script:
- ✅ Runs `cargo fmt --all --check`
- ✅ Runs `cargo clippy` on our crates
- ✅ Runs `cargo test` on our crates
- ✅ Works around the build script issue automatically

## Why This Works

Setting `CALIMERO_WEBUI_SRC` to a local path causes the build script to skip network requests and use the local directory instead. Since we're only running tests and checks (not the actual server), the dummy directory is sufficient.

## Long-Term Fix

The proper fix would be to:
1. Make the build scripts optional (feature-gated)
2. Fix the `reqwest` macOS issue upstream
3. Cache the assets in CI/CD environments

For now, the workaround is clean and reliable.


#!/usr/bin/env bash
# Puts `mero-app-build` on PATH (cargo's bin directory, or $CARGO_INSTALL_ROOT/bin).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo install --path crates/app-build --locked --force

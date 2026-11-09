#!/usr/bin/env bash
# Puts `mero-compute` on PATH (cargo's bin directory, or $CARGO_INSTALL_ROOT/bin).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo install --path tools/mero-compute --locked --force

#!/usr/bin/env bash
# Build cargo-mero and print the directory it landed in, so a caller can put that
# on PATH and then invoke the tool as `cargo mero <cmd>`:
#
#   PATH="$(scripts/setup-cargo-mero.sh):$PATH"
#   cargo mero build --manifest-path apps/kv-store/Cargo.toml
#
# CI's setup-cargo-mero action restores a cargo-mero built from these exact sources
# and sets CARGO_MERO_BIN_DIR to it; that directory is handed back without building.
set -euo pipefail

if [ -n "${CARGO_MERO_BIN_DIR:-}" ] && [ -x "$CARGO_MERO_BIN_DIR/cargo-mero" ]; then
    echo "$CARGO_MERO_BIN_DIR"
    exit 0
fi

cd "$(git rev-parse --show-toplevel)"
cargo build -q -p cargo-mero
cd "${CARGO_TARGET_DIR:-target}/debug" && pwd

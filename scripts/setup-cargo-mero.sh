#!/usr/bin/env bash
# Build cargo-mero and print the directory it landed in, so a caller can put that
# on PATH and then invoke the tool as `cargo mero <cmd>`:
#
#   PATH="$(scripts/setup-cargo-mero.sh):$PATH"
#   cargo mero build --manifest-path apps/kv-store/Cargo.toml
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
cargo build -q -p cargo-mero
cd "${CARGO_TARGET_DIR:-target}/debug" && pwd

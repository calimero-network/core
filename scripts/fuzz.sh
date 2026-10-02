#!/usr/bin/env bash
# Run one fuzz target until it crashes, or for [seconds]:
#
#   scripts/fuzz.sh bundle_open 600
set -euo pipefail

CARGO_FUZZ_VERSION=0.13.2 # pinned so CI and local runs build targets the same way

target=${1:?usage: scripts/fuzz.sh <target> [seconds] [libfuzzer flags...]}
seconds=${2:-0}
cd "$(git rev-parse --show-toplevel)"

rustup toolchain list | grep -q '^nightly' || rustup toolchain install nightly --profile minimal
command -v cargo-fuzz >/dev/null || cargo +nightly install cargo-fuzz --version "$CARGO_FUZZ_VERSION" --locked
cp Cargo.lock fuzz/Cargo.lock

mkdir -p "fuzz/corpus/$target"
seeds=()
[ -d "fuzz/seeds/$target" ] && seeds=("fuzz/seeds/$target")
cargo +nightly fuzz build "$target"
run=(cargo +nightly fuzz run "$target" "fuzz/corpus/$target" ${seeds[@]+"${seeds[@]}"} --
    -max_total_time="$seconds" "${@:3}")
# With FUZZ_LOG set, the run's output, which shows any failing input, goes there instead.
if [ -n "${FUZZ_LOG:-}" ]; then "${run[@]}" >"$FUZZ_LOG" 2>&1; else "${run[@]}"; fi

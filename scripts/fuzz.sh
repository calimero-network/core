#!/usr/bin/env bash
# Run one fuzz target until it crashes, or for [seconds]:
#
#   scripts/fuzz.sh bundle_open 600
set -euo pipefail

target=${1:?usage: scripts/fuzz.sh <target> [seconds] [libfuzzer flags...]}
seconds=${2:-0}
cd "$(git rev-parse --show-toplevel)"

rustup toolchain list | grep -q '^nightly' || rustup toolchain install nightly --profile minimal
command -v cargo-fuzz >/dev/null || cargo +nightly install cargo-fuzz --locked
cp Cargo.lock fuzz/Cargo.lock

mkdir -p "fuzz/corpus/$target"
seeds=()
[ -d "fuzz/seeds/$target" ] && seeds=("fuzz/seeds/$target")
cargo +nightly fuzz run "$target" "fuzz/corpus/$target" ${seeds[@]+"${seeds[@]}"} -- \
    -max_total_time="$seconds" "${@:3}"

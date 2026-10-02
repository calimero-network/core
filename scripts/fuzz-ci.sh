#!/usr/bin/env bash
# scripts/fuzz.sh for CI, whose log is public: a failing input is named only by
# its target and SHA-256, never shown or uploaded.
set -euo pipefail

target=${1:?usage: scripts/fuzz-ci.sh <target> <seconds>}
seconds=${2:?usage: scripts/fuzz-ci.sh <target> <seconds>}
cd "$(git rev-parse --show-toplevel)"

FUZZ_LOG=$(mktemp) scripts/fuzz.sh "$target" "$seconds" && exit 0

shopt -s nullglob
inputs=(fuzz/artifacts/"$target"/*)
[ ${#inputs[@]} -eq 0 ] && echo "::error::fuzz target $target failed without writing an input"
for input in "${inputs[@]}"; do
    echo "::error::fuzz target $target failed on an input with SHA-256 $(sha256sum "$input" | cut -d' ' -f1)"
done
exit 1

#!/usr/bin/env bash
# `meroctl app codegen` from this tree (calimero-app-build's codegen example), for the SDKs'
# vendored app clients: the same code meroctl runs, without building meroctl.
set -euo pipefail
exec cargo run -q --manifest-path "$(dirname "$0")/../Cargo.toml" -p calimero-app-build --example codegen -- "$@"

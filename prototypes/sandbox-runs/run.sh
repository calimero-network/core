#!/bin/sh
# Builds the prototype and runs every task: demo, escape suite, cross-run isolation, bench.
# Needs Linux, root (namespaces, mounts, cgroup v1), Rust with the wasm32-wasip2 target.
# Usage: ./run.sh [out.json]       SANDBOX_PROTO_CONTROL=no-seccomp|root ./run.sh  (controls)
set -eu
cd "$(dirname "$0")"
cargo build -q -p sandbox-proto-driver --release --target wasm32-wasip2
cargo build -q -p sandbox-proto-host --release
./target/release/sandbox-proto-host target/wasm32-wasip2/release/sandbox_proto_driver.wasm all "${1:-results.json}" >/dev/null
python3 -I summarize.py < "${1:-results.json}"

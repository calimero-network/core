#!/usr/bin/env bash
# R3 roadmap: docs/context-management/LOCAL-GROUP-GOVERNANCE.md §11.6 / §11.9.
# Lists inverse dependency paths from `merod` into key `near-*` crates.
set -euo pipefail
cd "$(dirname "$0")/.."

run_tree() {
  local crate="$1"
  echo ""
  echo "=== cargo tree -p merod -i ${crate} ==="
  cargo tree -p merod -i "${crate}" 2>&1 || echo "(not linked from merod — command may exit 1)"
}

run_tree near-primitives
run_tree near-jsonrpc-client
run_tree near-jsonrpc-primitives
run_tree near-crypto

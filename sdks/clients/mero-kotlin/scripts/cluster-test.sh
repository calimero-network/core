#!/usr/bin/env bash
# mero-kotlin's cluster tests (mero-core/src/test/.../cluster) against a real
# merod 1.0 cluster: starts `merod local` (three nodes, mock attestation) from
# MEROD_BIN on a scratch directory, runs the tests, and stops it.
#
#   MEROD_BIN=path/to/merod MERO_KV_MPK=path/to/kv-store-test-fixture.mpk \
#     scripts/cluster-test.sh [gradle args...]
#
# MERO_KV_MPK is core's kv-store test fixture (a signed package); its app.wasm
# is the raw component the tests deploy. MEROD_BASE_PORT (default 3500) puts
# the nodes on base+1..base+3. With MERO_CLUSTER already set (a running
# cluster's node URLs, comma-separated) no cluster is started.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
die() { echo "cluster-test: $*" >&2; exit 1; }

[ -n "${MERO_KV_MPK:-}" ] && [ -f "$MERO_KV_MPK" ] || die "set MERO_KV_MPK to core's kv-store test fixture (.mpk)"
WORK="$(mktemp -d)"
PID=""
cleanup() {
  local status=$?
  if [ -n "$PID" ]; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
  exit "$status"
}
trap cleanup EXIT

mkdir -p "$WORK/kv"
tar -xzf "$MERO_KV_MPK" -C "$WORK/kv" app.wasm
export MERO_KV_WASM="$WORK/kv/app.wasm"

if [ -z "${MERO_CLUSTER:-}" ]; then
  [ -n "${MEROD_BIN:-}" ] && [ -x "$MEROD_BIN" ] || die "set MEROD_BIN to a merod 1.0 binary (or MERO_CLUSTER to a running cluster)"
  BASE="${MEROD_BASE_PORT:-3500}"
  "$MEROD_BIN" local --base-port "$BASE" --dir "$WORK/cluster" --fresh >"$WORK/merod.log" 2>&1 &
  PID=$!
  for _ in $(seq 1 120); do
    curl -fsS -m 2 "http://127.0.0.1:$((BASE + 1))/health" 2>/dev/null | grep -q '"systemLeader":[1-9]' && break
    kill -0 "$PID" 2>/dev/null || { cat "$WORK/merod.log" >&2; die "merod local exited"; }
    sleep 1
  done
  export MERO_CLUSTER="http://127.0.0.1:$((BASE + 1)),http://127.0.0.1:$((BASE + 2)),http://127.0.0.1:$((BASE + 3))"
  echo "cluster-test: $("$MEROD_BIN" --version) on $MERO_CLUSTER"
fi

cd "$ROOT"
./gradlew :mero-core:testDebugUnitTest --tests 'com.calimero.mero.cluster.*' --rerun-tasks "$@"

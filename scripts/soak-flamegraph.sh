#!/usr/bin/env bash
# Profile one merod node through the kv-store soak with `perf record -g`, and
# render what it spent its CPU on as a flamegraph with inferno.
#
#   MEROD_BINARY=target/profiling/merod scripts/soak-flamegraph.sh OUT_DIR [NODE]
#
# Runs the process suite's kv-soak (crates/node/tests/fault/suite.py --only
# kv-soak, one `merod run --dev` process per node), finds node NODE (default 1)
# by the port it listens on (kv-soak's base port, 7650, + WORKFLOW_PORT_OFFSET
# + NODE), lets the setup pass (PROFILE_DELAY seconds, default 15), then
# samples that process and its threads at PROFILE_HZ (default 199) for
# PROFILE_SECONDS (default: the soak, SOAK_SECONDS, default 300, less the
# delay and 5 s). Writes to OUT_DIR:
#
#   merod-flamegraph.svg  the flamegraph (inferno-flamegraph)
#   merod.folded          the folded stacks it was drawn from (inferno-diff-folded
#                         compares two nights)
#   soak.json             the run's soak report (soak.py's format)
#   suite.log, suite.json
#
# Needs perf (or PERF=/path/to/perf) with kernel.perf_event_paranoid <= 1 (CI
# sets -1), inferno-collapse-perf and inferno-flamegraph on PATH, meroctl
# (MEROCTL_BINARY, default target/release/meroctl) and kv_store.wasm
# (KV_STORE_WASM, default target/wasm32-unknown-unknown/app-release). Build merod
# with `--profile profiling` and `-C force-frame-pointers=yes` for whole stacks;
# `-g` walks frame pointers. Exits with the suite's status.

set -euo pipefail

out=${1:?usage: soak-flamegraph.sh OUT_DIR [NODE]}
root=$(cd "$(dirname "$0")/.." && pwd)
node=${2:-1}
perf=${PERF:-perf}
target=${CARGO_TARGET_DIR:-$root/target}
merod=${MEROD_BINARY:-$target/profiling/merod}
meroctl=${MEROCTL_BINARY:-$target/release/meroctl}
# MEROBOX_FUZZY_SECONDS: the name this script read before the suite replaced merobox.
export SOAK_SECONDS=${SOAK_SECONDS:-${MEROBOX_FUZZY_SECONDS:-300}}
delay=${PROFILE_DELAY:-15}
seconds=${PROFILE_SECONDS:-$((SOAK_SECONDS - delay - 5))}
if [ "$seconds" -lt 1 ]; then
  echo "nothing to profile: SOAK_SECONDS=$SOAK_SECONDS leaves ${seconds}s after a ${delay}s delay" >&2
  exit 2
fi

mkdir -p "$out"
rm -f "$out/soak.json"
port=$((7650 + ${WORKFLOW_PORT_OFFSET:-0} + node))

python3 -I "$root/crates/node/tests/fault/suite.py" --only kv-soak \
  --merod "$merod" --meroctl "$meroctl" \
  --soak-seconds "$SOAK_SECONDS" --soak-report "$out/soak.json" --json "$out/suite.json" \
  >"$out/suite.log" 2>&1 &
runner=$!

# `merod run ... --listen 127.0.0.1:<port> --public-url ...`: one process per node.
pid=""
for _ in $(seq 1 600); do
  pid=$(pgrep -f -- "--listen 127.0.0.1:$port " | head -n1 || true)
  [ -n "$pid" ] && break
  kill -0 "$runner" 2>/dev/null || break
  sleep 0.1
done
if [ -z "$pid" ]; then
  echo "node $node (127.0.0.1:$port) never started:" >&2
  cat "$out/suite.log" >&2
  wait "$runner" || true
  exit 1
fi

echo "profiling merod node $node (pid $pid, port $port) for ${seconds}s after ${delay}s"
sleep "$delay"
"$perf" record -F "${PROFILE_HZ:-199}" -g -p "$pid" -o "$out/perf.data" -- sleep "$seconds"

status=0
wait "$runner" || status=$?
cat "$out/suite.log"

"$perf" script -i "$out/perf.data" 2>/dev/null | inferno-collapse-perf >"$out/merod.folded"
inferno-flamegraph --title "merod node $node, kv-store soak, ${seconds}s" \
  <"$out/merod.folded" >"$out/merod-flamegraph.svg"
# perf.data is large and names this machine's paths; the folded stacks carry it.
rm -f "$out/perf.data" "$out/perf.data.old"
echo "wrote $out/merod-flamegraph.svg ($(wc -l <"$out/merod.folded") stacks)"
exit "$status"

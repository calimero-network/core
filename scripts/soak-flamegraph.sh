#!/usr/bin/env bash
# Profile one merod node through a soak (fuzzy) workflow with `perf record -g`,
# and render what it spent its CPU on as a flamegraph with inferno.
#
#   MEROD_BINARY=target/profiling/merod scripts/soak-flamegraph.sh OUT_DIR [WORKFLOW] [NODE]
#
# Runs WORKFLOW (default: the kv-store fuzzy load) through
# scripts/run-workflows.py, finds node NODE (default 1) of its cluster by
# the port it listens on (the workflow's `cluster.base_port` + NODE), lets the
# workflow's setup pass (PROFILE_DELAY seconds, default 15), then samples that
# process and its threads at PROFILE_HZ (default 199) for PROFILE_SECONDS
# (default: the fuzzy run, MEROBOX_FUZZY_SECONDS, default 300, less the delay
# and 5 s). Writes to OUT_DIR:
#
#   merod-flamegraph.svg  the flamegraph (inferno-flamegraph)
#   merod.folded          the folded stacks it was drawn from (inferno-diff-folded
#                         compares two nights)
#   soak.json             the run's soak report (MEROBOX_SOAK_REPORT)
#   workflow.log, workflow.json
#
# Needs perf (or PERF=/path/to/perf) with kernel.perf_event_paranoid <= 1 (CI
# sets -1), inferno-collapse-perf and inferno-flamegraph on PATH, merobox
# installed, and the fixtures the workflow installs (CORE_WASM_DIR). Build merod
# with `--profile profiling` and `-C force-frame-pointers=yes` for whole stacks;
# `-g` walks frame pointers. Exits with the workflow's status.

set -euo pipefail

out=${1:?usage: soak-flamegraph.sh OUT_DIR [WORKFLOW] [NODE]}
root=$(cd "$(dirname "$0")/.." && pwd)
workflow=${2:-$root/workflows/fuzzy-tests/kv-store/fuzzy-test.yml}
node=${3:-1}
perf=${PERF:-perf}
export MEROBOX_FUZZY_SECONDS=${MEROBOX_FUZZY_SECONDS:-300}
delay=${PROFILE_DELAY:-15}
seconds=${PROFILE_SECONDS:-$((MEROBOX_FUZZY_SECONDS - delay - 5))}
if [ "$seconds" -lt 1 ]; then
  echo "nothing to profile: MEROBOX_FUZZY_SECONDS=$MEROBOX_FUZZY_SECONDS leaves ${seconds}s after a ${delay}s delay" >&2
  exit 2
fi

mkdir -p "$out"
base=$(python3 -c 'import sys, yaml; print(yaml.safe_load(open(sys.argv[1]))["cluster"]["base_port"])' "$workflow")
port=$((base + node))

MEROBOX_SOAK_REPORT="$out/soak.json" \
  python3 "$root/scripts/run-workflows.py" --json "$out/workflow.json" "$workflow" \
  >"$out/workflow.log" 2>&1 &
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
  cat "$out/workflow.log" >&2
  wait "$runner" || true
  exit 1
fi

echo "profiling merod node $node (pid $pid, port $port) for ${seconds}s after ${delay}s"
sleep "$delay"
"$perf" record -F "${PROFILE_HZ:-199}" -g -p "$pid" -o "$out/perf.data" -- sleep "$seconds"

status=0
wait "$runner" || status=$?
cat "$out/workflow.log"

"$perf" script -i "$out/perf.data" 2>/dev/null | inferno-collapse-perf >"$out/merod.folded"
inferno-flamegraph --title "merod node $node, $(basename "$(dirname "$workflow")") soak, ${seconds}s" \
  <"$out/merod.folded" >"$out/merod-flamegraph.svg"
# perf.data is large and names this machine's paths; the folded stacks carry it.
rm -f "$out/perf.data" "$out/perf.data.old"
echo "wrote $out/merod-flamegraph.svg ($(wc -l <"$out/merod.folded") stacks)"
exit "$status"

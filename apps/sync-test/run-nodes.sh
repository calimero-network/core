#!/usr/bin/env bash
#
# Run 3 independent merod nodes with full meroctl control.
#
# Usage:
#   ./run-nodes.sh          # Start all 3 nodes
#   ./run-nodes.sh stop     # Stop all nodes
#   ./run-nodes.sh clean    # Stop + delete data
#
# Nodes:
#   Node 1 (Fran):  server=2428 swarm=2528
#   Node 2 (Matea): server=2429 swarm=2529
#   Node 3 (Sandi): server=2430 swarm=2530
#
# meroctl usage:
#   alias mc1="../../target/debug/meroctl --node-url http://localhost:2428 --output-format json"
#   alias mc2="../../target/debug/meroctl --node-url http://localhost:2429 --output-format json"
#   alias mc3="../../target/debug/meroctl --node-url http://localhost:2430 --output-format json"

set -e
cd "$(dirname "$0")"

MEROD="../../target/debug/merod"
MEROCTL="../../target/debug/meroctl"
DATA="./data"
LOG_LEVEL="${RUST_LOG:-info,calimero_node::sync=debug,calimero_node::handlers=debug}"

N1_SERVER=2428; N1_SWARM=2528
N2_SERVER=2429; N2_SWARM=2529
N3_SERVER=2430; N3_SWARM=2530

stop_all() {
    echo "Stopping all nodes..."
    for pidfile in "$DATA"/n*.pid; do
        [ -f "$pidfile" ] || continue
        pid=$(cat "$pidfile")
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            echo "  Stopped pid $pid"
        fi
        rm -f "$pidfile"
    done
    sleep 1
    pkill -f "merod.*--home.*data.*--node n" 2>/dev/null || true
    echo "Done."
}

if [ "${1:-}" = "stop" ]; then
    stop_all
    exit 0
fi

if [ "${1:-}" = "clean" ]; then
    stop_all
    echo "Cleaning data..."
    rm -rf "$DATA"
    echo "Done."
    exit 0
fi

# Stop any running nodes first
stop_all
mkdir -p "$DATA"

# Init nodes
for spec in "n1:$N1_SERVER:$N1_SWARM" "n2:$N2_SERVER:$N2_SWARM" "n3:$N3_SERVER:$N3_SWARM"; do
    IFS=: read -r node sport swport <<< "$spec"
    if [ ! -d "$DATA/$node/$node" ]; then
        echo "Initializing $node (server=$sport, swarm=$swport)..."
        $MEROD --home "$DATA" --node "$node" init \
            --server-port "$sport" \
            --swarm-port "$swport" \
            --force 2>&1 | tail -1
    fi
done

# Start n1 first to get its peer ID
echo ""
echo "Starting n1 to get peer ID..."
mkdir -p "$DATA/n1/logs"
RUST_LOG="$LOG_LEVEL" $MEROD --home "$DATA" --node n1 run \
    > "$DATA/n1/logs/n1_stdout.log" 2>&1 &
N1_PID=$!
echo "$N1_PID" > "$DATA/n1.pid"
echo "  n1 started (pid=$N1_PID)"

# Wait for n1 to log its peer ID
echo "  Waiting for n1 to start..."
sleep 5

N1_PEER_ID=""
for i in $(seq 1 10); do
    N1_PEER_ID=$(grep -oE '12D3Koo[A-Za-z0-9]+' "$DATA/n1/logs/n1_stdout.log" 2>/dev/null | head -1 || true)
    if [ -n "$N1_PEER_ID" ]; then
        break
    fi
    sleep 1
done

if [ -n "$N1_PEER_ID" ]; then
    echo "  n1 Peer ID: $N1_PEER_ID"
else
    echo "  WARNING: Could not find n1 peer ID. Using mDNS discovery."
fi

# Start n2 and n3 with bootstrap pointing to n1
for spec in "n2:$N2_SERVER:$N2_SWARM" "n3:$N3_SERVER:$N3_SWARM"; do
    IFS=: read -r node sport swport <<< "$spec"
    mkdir -p "$DATA/$node/logs"

    if [ -n "$N1_PEER_ID" ]; then
        $MEROD --home "$DATA" --node "$node" config \
            "bootstrap.nodes=[\"/ip4/127.0.0.1/tcp/$N1_SWARM/p2p/$N1_PEER_ID\"]" 2>/dev/null || true
    fi

    RUST_LOG="$LOG_LEVEL" $MEROD --home "$DATA" --node "$node" run \
        > "$DATA/$node/logs/${node}_stdout.log" 2>&1 &
    pid=$!
    echo "$pid" > "$DATA/$node.pid"
    echo "  $node started (pid=$pid, server=localhost:$sport)"
done

sleep 3

echo ""
echo "============================================"
echo "All 3 nodes running!"
echo ""
echo "Set up shortcuts:"
echo ""
echo "  mc1='$MEROCTL --node-url http://localhost:$N1_SERVER --output-format json'"
echo "  mc2='$MEROCTL --node-url http://localhost:$N2_SERVER --output-format json'"
echo "  mc3='$MEROCTL --node-url http://localhost:$N3_SERVER --output-format json'"
echo ""
echo "Quick test:"
echo "  \$mc1 app install --path res/sync_test.wasm"
echo ""
echo "Logs:"
echo "  tail -f $DATA/n1/logs/n1_stdout.log"
echo "  tail -f $DATA/n2/logs/n2_stdout.log"
echo "  tail -f $DATA/n3/logs/n3_stdout.log"
echo ""
echo "Stop:  ./run-nodes.sh stop"
echo "Clean: ./run-nodes.sh clean"
echo "============================================"

# Keep script alive waiting for children
wait

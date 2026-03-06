#!/bin/bash
set -uo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
MEROD="$DIR/../../target/release/merod"
BOT="$DIR/../../target/release/poker-bot"

trap 'echo ""; echo "Stopping..."; kill $(jobs -p) 2>/dev/null; wait 2>/dev/null; cd "$DIR" && merobox nuke 2>/dev/null' EXIT

cat > /tmp/_poker_setup.yml << 'EOF'
name: 5-Player Setup
force_pull_image: true
nodes:
  chain_id: testnet-1
  count: 5
  image: ghcr.io/calimero-network/merod:edge
  prefix: p
steps:
  - {name: Install, type: install_application, node: p-1, path: res/poker.wasm, dev: true, outputs: {app_id: applicationId}}
  - {name: Mesh, type: create_mesh, context_node: p-1, application_id: "{{app_id}}", params: '{"small_blind":5,"big_blind":10,"min_buy_in":50}', nodes: [p-2, p-3, p-4, p-5], capability: member, outputs: {context_id: contextId, member_public_key: memberPublicKey}}
  - {name: J1, type: call, node: p-1, context_id: "{{context_id}}", executor_public_key: "{{member_public_key}}", method: join_table, args: {buy_in: 200}}
  - {name: S1, type: wait_for_sync, context_id: "{{context_id}}", nodes: [p-1, p-2, p-3, p-4, p-5], timeout: 120, check_interval: 3, trigger_sync: true}
  - {name: J2, type: call, node: p-2, context_id: "{{context_id}}", executor_public_key: "{{public_key_p-2}}", method: join_table, args: {buy_in: 200}}
  - {name: S2, type: wait_for_sync, context_id: "{{context_id}}", nodes: [p-1, p-2, p-3, p-4, p-5], timeout: 60, check_interval: 2, trigger_sync: true}
  - {name: J3, type: call, node: p-3, context_id: "{{context_id}}", executor_public_key: "{{public_key_p-3}}", method: join_table, args: {buy_in: 200}}
  - {name: S3, type: wait_for_sync, context_id: "{{context_id}}", nodes: [p-1, p-2, p-3, p-4, p-5], timeout: 60, check_interval: 2, trigger_sync: true}
  - {name: J4, type: call, node: p-4, context_id: "{{context_id}}", executor_public_key: "{{public_key_p-4}}", method: join_table, args: {buy_in: 200}}
  - {name: S4, type: wait_for_sync, context_id: "{{context_id}}", nodes: [p-1, p-2, p-3, p-4, p-5], timeout: 60, check_interval: 2, trigger_sync: true}
  - {name: J5, type: call, node: p-5, context_id: "{{context_id}}", executor_public_key: "{{public_key_p-5}}", method: join_table, args: {buy_in: 200}}
  - {name: S5, type: wait_for_sync, context_id: "{{context_id}}", nodes: [p-1, p-2, p-3, p-4, p-5], timeout: 60, check_interval: 2, trigger_sync: true}
  - {name: READY, type: wait, seconds: 600}
stop_all_nodes: false
wait_timeout: 700
EOF

echo ""
echo "♠♥♣♦  WSOP FINAL TABLE — 5 PLAYERS  ♦♣♥♠"
echo "  🦈 SHARK1 (TAG)  🐺 SHARK2 (TAG)  📞 STATION (Caller)"
echo "  🎲 GAMBLER (Random)  🎰 WILDCARD (Random)"
echo "  Buy-in: 200  Blinds: 5/10"
echo ""
echo "Setting up 5-node table..."

cd "$DIR"
merobox bootstrap run /tmp/_poker_setup.yml \
  --no-docker --binary-path "$MEROD" --e2e-mode -v > /tmp/_poker.log 2>&1 &

while ! grep -q "READY" /tmp/_poker.log 2>/dev/null; do sleep 1; done

CTX=$(grep "context_id = " /tmp/_poker.log | head -1 | awk '{print $NF}')
K1=$(grep "member_public_key = " /tmp/_poker.log | head -1 | awk '{print $NF}')
K2=$(grep "executor_public_key:" /tmp/_poker.log | sed -n '2p' | awk '{print $NF}')
K3=$(grep "executor_public_key:" /tmp/_poker.log | sed -n '3p' | awk '{print $NF}')
K4=$(grep "executor_public_key:" /tmp/_poker.log | sed -n '4p' | awk '{print $NF}')
K5=$(grep "executor_public_key:" /tmp/_poker.log | sed -n '5p' | awk '{print $NF}')
P1=$(grep "RPC port" /tmp/_poker.log | sed -n '1p' | awk '{print $NF}')
P2=$(grep "RPC port" /tmp/_poker.log | sed -n '2p' | awk '{print $NF}')
P3=$(grep "RPC port" /tmp/_poker.log | sed -n '3p' | awk '{print $NF}')
P4=$(grep "RPC port" /tmp/_poker.log | sed -n '4p' | awk '{print $NF}')
P5=$(grep "RPC port" /tmp/_poker.log | sed -n '5p' | awk '{print $NF}')

echo "✓ Table live ($CTX)"
echo ""

# Player 1 is the reporter (prints results + scoreboards)
$BOT --node http://localhost:$P1 --context $CTX --key $K1 --strategy tag    --buy-in 0 --poll-ms 2000 --name SHARK1   --reporter &
$BOT --node http://localhost:$P2 --context $CTX --key $K2 --strategy tag    --buy-in 0 --poll-ms 2000 --name SHARK2   &
$BOT --node http://localhost:$P3 --context $CTX --key $K3 --strategy caller --buy-in 0 --poll-ms 2000 --name STATION  &
$BOT --node http://localhost:$P4 --context $CTX --key $K4 --strategy random --buy-in 0 --poll-ms 2000 --name GAMBLER  &
$BOT --node http://localhost:$P5 --context $CTX --key $K5 --strategy random --buy-in 0 --poll-ms 2000 --name WILDCARD &

echo "🤖 5 bots playing. Ctrl+C to stop."
echo ""
wait

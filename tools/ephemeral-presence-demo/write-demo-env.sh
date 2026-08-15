#!/bin/sh
# Capture the ids the workflow just produced and print the exact commands to run.
#
# The context id is generated per run, so the README cannot hard-code it. The
# workflow writes it here instead, and every command in the README sources this
# file — so the copy-pasteable lines stay literal.
#
# Args: $1 node1_url  $2 node2_url  $3 context_id  $4 node1_key  $5 node2_key
set -eu

NODE1_URL="$1"
NODE2_URL="$2"
CONTEXT_ID="$3"
NODE1_KEY="$4"
NODE2_KEY="$5"

OUT="tools/ephemeral-presence-demo/.demo-env"

if [ -z "$CONTEXT_ID" ]; then
  echo "ERROR: context id is empty — the create_context step produced no output" >&2
  exit 1
fi

cat > "$OUT" <<EOF
# Written by tools/ephemeral-presence-demo/workflow.yml — regenerated every run.
export NODE1_URL="$NODE1_URL"
export NODE2_URL="$NODE2_URL"
export CONTEXT_ID="$CONTEXT_ID"
export NODE1_KEY="$NODE1_KEY"
export NODE2_KEY="$NODE2_KEY"
EOF

echo ""
echo "=========================================================================="
echo " ephemeral presence demo is UP — both nodes are still running"
echo "=========================================================================="
echo "  node 1 : $NODE1_URL   member $NODE1_KEY"
echo "  node 2 : $NODE2_URL   member $NODE2_KEY"
echo "  context: $CONTEXT_ID"
echo "  wrote  : $OUT"
echo ""
echo "Now open three terminals, and in EACH of them first run:"
echo "  source tools/ephemeral-presence-demo/.demo-env"
echo ""
echo "Terminal 1 (alice, on node 1):"
echo "  node tools/ephemeral-presence-demo/cursor-client.js --node \$NODE1_URL --context \$CONTEXT_ID --name alice --label node-1"
echo ""
echo "Terminal 2 (bob, on node 2):"
echo "  node tools/ephemeral-presence-demo/cursor-client.js --node \$NODE2_URL --context \$CONTEXT_ID --name bob --label node-2"
echo ""
echo "Terminal 3 (a late watcher — start it LAST, it is seeded on subscribe):"
echo "  node tools/ephemeral-presence-demo/cursor-client.js --node \$NODE2_URL --context \$CONTEXT_ID --watch --name watcher --label node-2"
echo ""
echo "Tear down when finished:  merobox nuke"
echo "=========================================================================="

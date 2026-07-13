#!/bin/sh
# wire-bootstrap.sh — patch node 2's config to include node 1 as a bootstrap peer.
#
# Called by merobox as a `script` step (target: local) immediately after both
# nodes have started. Node 2 is then stopped + restarted (stop_node / start_node
# steps in the workflow) so it picks up the updated bootstrap.nodes.
#
# Background: binary-mode merobox does NOT wire sibling multiaddrs the way the
# Docker manager does. Without this patch, nodes rely solely on mDNS, which can
# take >120 s on macOS with VPN interfaces — too slow for CI.
#
# The script derives the multiaddr from node 1's config.toml (written by merod
# during init) and inserts it into node 2's bootstrap.nodes. Both config paths
# are hard-coded to match the fixed ports defined in the workflow (base_port=8830,
# prefix=ephemeral-node).
#
# Exit 0 = success. Exit 1 = could not read peer_id (fatal, stops workflow).

set -eu

# Triple-nested path mirrors merobox's native (no-docker) data layout:
#   data/<prefix>/<name>/<name>/config.toml  — i.e. data/<node>/<node>/<node>.
NODE1_CONFIG="data/ephemeral-node-1/ephemeral-node-1/ephemeral-node-1/config.toml"
NODE2_CONFIG="data/ephemeral-node-2/ephemeral-node-2/ephemeral-node-2/config.toml"
NODE1_P2P_PORT="8830"

echo "=== wire-bootstrap: patching node 2 bootstrap with node 1 peer address ==="

if [ ! -f "$NODE1_CONFIG" ]; then
  echo "ERROR: node 1 config not found at $NODE1_CONFIG"
  exit 1
fi

if [ ! -f "$NODE2_CONFIG" ]; then
  echo "ERROR: node 2 config not found at $NODE2_CONFIG"
  exit 1
fi

# Extract node 1's peer_id using Python (reliable TOML-aware extraction).
NODE1_PEER_ID=$(python3 -c "
import re, sys
path = '$NODE1_CONFIG'
with open(path) as f:
    content = f.read()
m = re.search(r'peer_id\s*=\s*\"([^\"]+)\"', content)
if m:
    print(m.group(1))
else:
    sys.exit(1)
")

if [ -z "$NODE1_PEER_ID" ]; then
  echo "ERROR: could not extract peer_id from $NODE1_CONFIG"
  exit 1
fi

MULTIADDR="/ip4/127.0.0.1/tcp/$NODE1_P2P_PORT/p2p/$NODE1_PEER_ID"
echo "  node 1 peer_id : $NODE1_PEER_ID"
echo "  bootstrap addr : $MULTIADDR"

# Patch node 2's config: insert the multiaddr into [bootstrap].nodes.
# The config format produced by merod init is always:
#   [bootstrap]
#   nodes = [
#       "/ip4/...",
#   ]
# We insert the new entry before the closing ].
python3 - <<PYEOF
import re, sys

config_path = '$NODE2_CONFIG'
multiaddr = '$MULTIADDR'

with open(config_path) as f:
    content = f.read()

# Check that the multiaddr is not already present (idempotent).
if multiaddr in content:
    print('  (multiaddr already present in node 2 config — skipping)')
    sys.exit(0)

# Match the [bootstrap] section's nodes array and append our entry.
# The DOTALL flag lets .* cross newlines.
pattern = r'(\[bootstrap\][^\[]*?nodes\s*=\s*\[)(.*?)(\])'

def insert_entry(m):
    prefix = m.group(1)
    existing = m.group(2)
    closing = m.group(3)
    new_entry = '\n    "' + multiaddr + '",'
    return prefix + existing + new_entry + '\n' + closing

new_content, n = re.subn(pattern, insert_entry, content, flags=re.DOTALL)
if n == 0:
    print('ERROR: could not find [bootstrap].nodes in node 2 config', file=sys.stderr)
    sys.exit(1)

with open(config_path, 'w') as f:
    f.write(new_content)

print('  patched: ' + config_path)
PYEOF

echo "=== wire-bootstrap: done ==="

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
# during init) and inserts it into node 2's bootstrap.nodes.
#
# Configuration comes from the environment, defaulting to the values in
# workflows/ephemeral-presence-e2e.yml (prefix `ephemeral-node`, base_port
# 8830). Override NODE_PREFIX / NODE1_P2P_PORT / DATA_ROOT if the workflow
# changes rather than editing paths in here.
#
# Requires python3 >= 3.11 for `tomllib`. An older interpreter fails the step
# outright, which is the intended behaviour: this script's job is to make node
# discovery deterministic, and a silently-skipped patch would leave the e2e on
# mDNS and look like a network flake.
#
# Exit 0 = success. Exit 1 = fatal (config not found, peer_id unreadable,
# patched config no longer parses as TOML) — stops the workflow.

set -eu

NODE_PREFIX="${NODE_PREFIX:-ephemeral-node}"
NODE1_NAME="${NODE1_NAME:-${NODE_PREFIX}-1}"
NODE2_NAME="${NODE2_NAME:-${NODE_PREFIX}-2}"
NODE1_P2P_PORT="${NODE1_P2P_PORT:-8830}"
DATA_ROOT="${DATA_ROOT:-data}"

echo "=== wire-bootstrap: patching node 2 bootstrap with node 1 peer address ==="

# Locate each node's config.toml by searching under its data directory instead
# of hard-coding merobox's current native layout (which today happens to nest
# the node name three deep: data/<name>/<name>/<name>/config.toml). A search
# keeps working if that layout changes; ambiguity is reported rather than
# silently resolved.
find_config() {
  node_name="$1"
  matches=$(find "$DATA_ROOT/$node_name" -name config.toml -type f 2>/dev/null || true)
  count=$(printf '%s' "$matches" | grep -c . || true)
  if [ "$count" -eq 0 ]; then
    echo "ERROR: no config.toml found under $DATA_ROOT/$node_name" >&2
    echo "       (set DATA_ROOT / NODE_PREFIX if the workflow's layout changed)" >&2
    return 1
  fi
  if [ "$count" -gt 1 ]; then
    echo "ERROR: $count config.toml files under $DATA_ROOT/$node_name — ambiguous:" >&2
    echo "$matches" >&2
    return 1
  fi
  printf '%s' "$matches"
}

NODE1_CONFIG=$(find_config "$NODE1_NAME")
NODE2_CONFIG=$(find_config "$NODE2_NAME")
echo "  node 1 config  : $NODE1_CONFIG"
echo "  node 2 config  : $NODE2_CONFIG"

# Extract node 1's peer_id by PARSING the TOML, not by regex: a regex would
# happily match a `peer_id` in a comment or in an unrelated table.
NODE1_PEER_ID=$(NODE1_CONFIG="$NODE1_CONFIG" python3 -c '
import os, sys, tomllib

with open(os.environ["NODE1_CONFIG"], "rb") as f:
    config = tomllib.load(f)


def find_peer_id(node):
    if isinstance(node, dict):
        value = node.get("peer_id")
        if isinstance(value, str) and value:
            return value
        for child in node.values():
            found = find_peer_id(child)
            if found:
                return found
    return None


peer_id = find_peer_id(config)
if not peer_id:
    sys.exit(1)
print(peer_id)
')

if [ -z "$NODE1_PEER_ID" ]; then
  echo "ERROR: could not extract peer_id from $NODE1_CONFIG" >&2
  exit 1
fi

MULTIADDR="/ip4/127.0.0.1/tcp/$NODE1_P2P_PORT/p2p/$NODE1_PEER_ID"
echo "  node 1 peer_id : $NODE1_PEER_ID"
echo "  bootstrap addr : $MULTIADDR"

# Patch node 2's config: append the multiaddr to [bootstrap].nodes.
#
# The edit is still textual — stdlib has a TOML reader (tomllib) but no writer,
# and rewriting the whole file from a parsed dict would discard merod's comments
# and formatting. What the regex CANNOT do is silently corrupt the file: the
# result is re-parsed with tomllib and the entry is asserted present in the
# parsed value before the write is kept. A malformed patch fails the step
# instead of producing a node that starts with no bootstrap peers and falls
# back to slow mDNS — the exact false-pass this script exists to prevent.
NODE2_CONFIG="$NODE2_CONFIG" MULTIADDR="$MULTIADDR" python3 -c '
import os, re, sys, tomllib

config_path = os.environ["NODE2_CONFIG"]
multiaddr = os.environ["MULTIADDR"]

with open(config_path) as f:
    content = f.read()


def bootstrap_nodes(text):
    """The [bootstrap].nodes list as TOML actually parses it."""
    parsed = tomllib.loads(text)
    section = parsed.get("bootstrap")
    if not isinstance(section, dict):
        return None
    nodes = section.get("nodes")
    return nodes if isinstance(nodes, list) else None


before = bootstrap_nodes(content)
if before is None:
    sys.exit("ERROR: node 2 config has no [bootstrap].nodes list")

if multiaddr in before:
    print("  (multiaddr already present in node 2 config — skipping)")
    sys.exit(0)

pattern = r"(\[bootstrap\][^\[]*?nodes\s*=\s*\[)(.*?)(\])"
patched, n = re.subn(
    pattern,
    lambda m: m.group(1) + m.group(2).rstrip() + f"\n    \"{multiaddr}\",\n" + m.group(3),
    content,
    flags=re.DOTALL,
)
if n != 1:
    sys.exit(f"ERROR: expected exactly one [bootstrap].nodes list, matched {n}")

# The patch is only trusted if the result still parses AND the entry is really
# in the list a TOML reader sees.
try:
    after = bootstrap_nodes(patched)
except tomllib.TOMLDecodeError as err:
    sys.exit(f"ERROR: patched config is not valid TOML ({err}) — refusing to write")

if after is None or multiaddr not in after:
    sys.exit("ERROR: patched config parses but does not contain the multiaddr — refusing to write")

with open(config_path, "w") as f:
    f.write(patched)

print(f"  patched: {config_path} ({len(before)} -> {len(after)} bootstrap nodes)")
'

echo "=== wire-bootstrap: done ==="

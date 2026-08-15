#!/bin/sh
# Point the shared bootstrap-wiring helper at the DEMO's node prefix and ports.
#
# workflows/scripts/wire-bootstrap.sh takes its node names and p2p port from the
# environment precisely so a second workflow can reuse it; this wrapper is that
# second workflow. Keeping one copy of the peer_id-parsing logic matters more
# than saving a file: if merobox's native data layout changes, only the shared
# helper has to follow.
#
# Run from the repo root (merobox executes script steps with cwd = repo root).
set -eu

NODE_PREFIX=presence-demo-node \
NODE1_P2P_PORT=8840 \
exec sh workflows/scripts/wire-bootstrap.sh "$@"

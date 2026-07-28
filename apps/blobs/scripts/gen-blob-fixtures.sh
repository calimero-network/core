#!/usr/bin/env bash
#
# gen-blob-fixtures.sh — make the large, RANDOM blob fixtures the cross-node
# size sweep uploads. Not committed: 141 MiB of binary has no business in git,
# and the content must differ per run anyway (see below).
#
# Usage (from apps/blobs, or anywhere — paths resolve off this script):
#   ./scripts/gen-blob-fixtures.sh                      # CI tier: 1 8 32 100 MiB
#   BLOB_FIXTURE_SIZES_MIB="1 50 250 500" ./scripts/gen-blob-fixtures.sh
#
# WHY RANDOM, NOT ZEROS: blobs are content-addressed. A fixture of predictable
# bytes hashes to the same blob id on every run and on every node, so a node
# that already holds it serves it from local storage and the "cross-node fetch"
# under test never touches the network — a green run proving nothing. Random
# content makes each run's blob genuinely absent on the fetching node.
#
# WHY THESE SIZES: 1 MiB is a single chunk (CHUNK_SIZE is 1 MiB), so anything
# above it exercises the chunk-graph walk — 8 MiB is 8 chunks, 32 and 100 push
# the streaming path. The default tier stops at 100 MiB to keep the CI job fast;
# the hard ceiling is 500 MiB (MAX_BLOB_SIZE_BYTES,
# crates/network/src/handlers/commands/request_blob.rs) and anything past it is
# refused by design, not broken.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/../static/generated"
SIZES_MIB="${BLOB_FIXTURE_SIZES_MIB:-1 8 32 100}"

mkdir -p "$OUT_DIR"

for mib in $SIZES_MIB; do
  target="$OUT_DIR/blob-${mib}mib.bin"
  want=$((mib * 1024 * 1024))

  # Regenerate every time rather than reusing a same-size file: a fixture kept
  # across runs would already be on the fetching node from the previous run.
  if [ -f "$target" ]; then
    rm -f "$target"
  fi

  echo ">>> generating ${mib} MiB -> $target"
  # `dd` from /dev/urandom in 1 MiB blocks. bs=1M is GNU/BSD-portable enough for
  # CI (ubuntu) and macOS dev boxes.
  dd if=/dev/urandom of="$target" bs=1048576 count="$mib" status=none

  got=$(wc -c < "$target" | tr -d ' ')
  if [ "$got" != "$want" ]; then
    echo "ERROR: $target is $got bytes, expected $want" >&2
    exit 1
  fi
done

echo ">>> blob fixtures ready in $OUT_DIR"
ls -l "$OUT_DIR"

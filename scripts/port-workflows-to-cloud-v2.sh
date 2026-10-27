#!/usr/bin/env bash
# Regenerate workflows/cloud-v2/ from the v1 workflows under workflows/, with
# merobox's converter (`merobox cloud port`, merobox branch release-v2-parity).
# The tree mirrors the sources: workflows/app-migration/00-x.yml ->
# workflows/cloud-v2/app-migration/00-x.yml, workflows/auth-seam.yml ->
# workflows/cloud-v2/auth-seam.yml.
#
# Hand edits to a port are lost on regeneration; scripts/cloud-v2-port-fixups.py
# re-applies them (each one names the converter bug it works around).
#
# MEROBOX: how to run merobox (default `merobox`; e.g.
#   MEROBOX="env PYTHONPATH=$HOME/merobox python3 -m merobox.cli").
# MEROBOX_PYTHON: a python that imports merobox (for the fixups; default python3).
set -euo pipefail
cd "$(dirname "$0")/.."
MEROBOX=${MEROBOX:-merobox}
map=$(mktemp)
trap 'rm -f "$map"' EXIT
python3 scripts/cloud-v2-wasm-map.py > "$map"
port=3000
port_dir() { # out-dir sources...
  local out=$1
  shift
  $MEROBOX cloud port "$@" --wasm-map "$map" --out-dir "$out" --base-port "$port" >/dev/null
  port=$((port + 10 * $#))
}
mapfile -t migration < <(find workflows/app-migration -maxdepth 1 -name '*.yml' | sort)
port_dir workflows/cloud-v2/app-migration "${migration[@]}"
for f in workflows/fuzzy-tests/*/fuzzy-test.yml; do
  port_dir "workflows/cloud-v2/fuzzy-tests/$(basename "$(dirname "$f")")" "$f"
done
mapfile -t sync < <(find workflows/sync-tests -maxdepth 1 -name '*.yml' | sort)
port_dir workflows/cloud-v2/sync-tests "${sync[@]}"
port_dir workflows/cloud-v2 workflows/auth-seam.yml
${MEROBOX_PYTHON:-python3} scripts/cloud-v2-port-fixups.py
echo "ported into workflows/cloud-v2/"

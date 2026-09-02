#!/usr/bin/env bash
# Serves the built app bundles over HTTP in the layout the downloader expects,
# so e2e nodes resolve applications the way a real deployment does.
set -euo pipefail

readonly CONTAINER=fixture-registry
readonly IMAGE=nginx:alpine

dist_dir="${1:-dist}"
root="${FIXTURE_REGISTRY_ROOT:-$PWD/fixture-registry}"
port="${FIXTURE_REGISTRY_PORT:-8080}"

# `{package}-{version}.mpk`, split on the dash that starts the version; the
# package half contains dashes of its own.
stage() {
  local mpk base dest
  rm -rf "$root"
  for mpk in "$dist_dir"/*.mpk; do
    base=$(basename "$mpk" .mpk)
    if [[ ! $base =~ ^(.+)-([0-9]+\.[0-9]+\.[0-9]+.*)$ ]]; then
      echo "bundle name is not {package}-{version}.mpk: $mpk" >&2
      return 1
    fi
    dest="$root/artifacts/${BASH_REMATCH[1]}/${BASH_REMATCH[2]}"
    mkdir -p "$dest"
    # Copied, never repacked: a rewritten bundle changes its bytecode id.
    cp "$mpk" "$dest/$base.mpk"
  done
  [ -d "$root/artifacts" ] || { echo "no bundles in $dist_dir" >&2; return 1; }
}

stage

if [ "${FIXTURE_REGISTRY_STAGE_ONLY:-}" = 1 ]; then
  exit 0
fi

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" -p "$port:80" \
  -v "$root:/usr/share/nginx/html:ro" "$IMAGE" >/dev/null

# Published on the host and addressed by the docker0 gateway: that address is
# the host itself, so it resolves from every bridge network, not just this one.
gateway=$(docker network inspect bridge -f '{{(index .IPAM.Config 0).Gateway}}')
[ -n "$gateway" ] || { echo "no gateway on the default bridge" >&2; exit 1; }
echo "http://$gateway:$port/"

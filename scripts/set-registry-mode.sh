#!/usr/bin/env bash
# merobox has no per-node env key, so a node's application source has to travel
# in the image it boots from. Retags IMAGE in place with that source.
set -euo pipefail

image=$1
mode=$2
base_url=${3:-}

{
  echo "FROM $image"
  echo "ENV CALIMERO_REGISTRY_MODE=$mode"
  [ -z "$base_url" ] || echo "ENV CALIMERO_REGISTRY_URL=$base_url"
} | docker build -q -t "$image" - >/dev/null

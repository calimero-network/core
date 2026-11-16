#!/usr/bin/env bash
# A local Calimero 1.0 development cluster (`merod local`: three nodes on
# 2501-2503, mock attestation) for testing mobile apps and the SDKs.
#
# merod 1.0 is Linux-only, so on a Mac this runs core's published image
# (ghcr.io/calimero-network/merod) in Docker. `merod local` listens on
# 127.0.0.1 inside the container, so a small forwarder sharing its network
# publishes the nodes on the Mac: localhost:2501-2503 for the iOS Simulator,
# 10.0.2.2 from the Android emulator, and the Mac's LAN address for a phone.
# With MEROD_BIN set (a merod 1.0 binary, e.g. on Linux) it runs that instead.
#
# The same copy of this script lives in swift-sdk, kotlin-sdk, mero-tag and
# mero-ar; they share one cluster (container `merod-local`, volume
# `merod-local`), so running `up` from any of them reuses it.
#
#   scripts/dev-cluster.sh up [--fresh]     start (or reuse) the cluster and print its URLs
#   scripts/dev-cluster.sh install FILE     install a .mpk (or a raw .wasm) as a dev account
#   scripts/dev-cluster.sh android          adb reverse 2501-2503, so a USB phone reaches localhost
#   scripts/dev-cluster.sh urls             where each simulator/device reaches the cluster
#   scripts/dev-cluster.sh status | logs | down
#
# CORE_TAG picks the merod release (default: ci/core-version's CORE_TAG when
# the repo has one, else the release below).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${CORE_TAG:-}" ] && [ -f "$ROOT/ci/core-version" ]; then
  CORE_TAG=$(sed -n 's/^CORE_TAG=//p' "$ROOT/ci/core-version" | head -1)
fi
CORE_TAG="${CORE_TAG:-1.0.0-rc.29}"
IMAGE="${MEROD_IMAGE:-ghcr.io/calimero-network/merod:${CORE_TAG}}"
FORWARDER_IMAGE="alpine/socat:1.8.0.3"
NAME="merod-local"
VOLUME="merod-local"
PORTS=(2501 2502 2503)
NATIVE_DIR="${MEROD_LOCAL_DIR:-$HOME/.merod-local}"

die() { echo "dev-cluster: $*" >&2; exit 1; }
say() { echo "dev-cluster: $*"; }

native() { [ -n "${MEROD_BIN:-}" ]; }

healthy() { curl -fsS -m 2 "http://127.0.0.1:${PORTS[0]}/health" >/dev/null 2>&1; }

wait_healthy() {
  for _ in $(seq 1 60); do
    healthy && return 0
    sleep 2
  done
  die "the cluster did not answer http://127.0.0.1:${PORTS[0]}/health within 120s (see: $0 logs)"
}

need_docker() {
  command -v docker >/dev/null 2>&1 || die "Docker is needed (Docker Desktop on a Mac), or set MEROD_BIN to a merod 1.0 binary"
  docker info >/dev/null 2>&1 || die "Docker is not running; start Docker Desktop"
}

lan_ip() {
  local ip=""
  if command -v ipconfig >/dev/null 2>&1; then
    for ifc in en0 en1 en2; do
      ip=$(ipconfig getifaddr "$ifc" 2>/dev/null || true)
      [ -n "$ip" ] && break
    done
  fi
  [ -z "$ip" ] && ip=$(hostname -I 2>/dev/null | awk '{print $1}' || true)
  echo "$ip"
}

urls() {
  local ip
  ip=$(lan_ip)
  [ -n "$ip" ] || ip="<your Mac LAN address>"
  cat <<EOF

  Calimero dev cluster (merod ${CORE_TAG}, mock attestation)

  iOS Simulator         http://localhost:2501
  Android emulator      http://10.0.2.2:2501
  Android phone (USB)   http://localhost:2501   after: $0 android
EOF
  if native; then
    echo "  iPhone (Wi-Fi)        not reachable: a native merod listens on 127.0.0.1 only"
  else
    echo "  iPhone (Wi-Fi)        http://${ip}:2501   same network; allow Local Network when asked"
  fi
  cat <<EOF

  In the app pick the development cluster (merod local) option: it trusts the
  mock attestation and signs in with a software passkey it keeps for the node.

EOF
}

up_docker() {
  local fresh="$1"
  need_docker
  if [ "$fresh" = 1 ]; then
    docker rm -f "$NAME" "$NAME-forward" >/dev/null 2>&1 || true
    docker volume rm "$VOLUME" >/dev/null 2>&1 || true
  fi
  if docker ps --format '{{.Names}}' | grep -qx "$NAME"; then
    local running
    running=$(docker inspect -f '{{.Config.Image}}' "$NAME")
    [ "$running" = "$IMAGE" ] || say "note: reusing the running cluster ($running); '$0 up --fresh' switches to $IMAGE"
  else
    docker rm -f "$NAME" "$NAME-forward" >/dev/null 2>&1 || true
    say "starting $IMAGE"
    # merod listens on 127.0.0.1:250x in the container; the forwarder listens on
    # 350x in the same network namespace, and Docker publishes 350x as 250x.
    docker run -d --name "$NAME" \
      -p 2501:3501 -p 2502:3502 -p 2503:3503 \
      -v "$VOLUME:/data" \
      "$IMAGE" local --dir /data/cluster >/dev/null
  fi
  if ! docker ps --format '{{.Names}}' | grep -qx "$NAME-forward"; then
    docker rm -f "$NAME-forward" >/dev/null 2>&1 || true
    docker run -d --name "$NAME-forward" --network "container:$NAME" \
      --entrypoint sh "$FORWARDER_IMAGE" -c \
      'for i in 1 2 3; do socat TCP-LISTEN:350$i,fork,reuseaddr TCP:127.0.0.1:250$i & done; wait' >/dev/null
  fi
}

up_native() {
  local fresh="$1"
  "$MEROD_BIN" --version 2>/dev/null | grep -qE '^merod 1\.' \
    || die "MEROD_BIN ($MEROD_BIN) is not merod 1.x: $("$MEROD_BIN" --version 2>&1 | head -1)"
  healthy && { say "a cluster already answers on :2501"; return; }
  mkdir -p "$NATIVE_DIR"
  local args=(local --dir "$NATIVE_DIR/cluster")
  [ "$fresh" = 1 ] && args+=(--fresh)
  say "starting $MEROD_BIN ${args[*]}"
  nohup "$MEROD_BIN" "${args[@]}" >"$NATIVE_DIR/merod.log" 2>&1 &
  echo $! >"$NATIVE_DIR/merod.pid"
}

cmd_up() {
  local fresh=0
  [ "${1:-}" = "--fresh" ] && fresh=1
  if native; then up_native "$fresh"; else up_docker "$fresh"; fi
  wait_healthy
  say "healthy: $(curl -fsS -m 2 "http://127.0.0.1:${PORTS[0]}/health")"
  urls
}

cmd_down() {
  if native; then
    [ -f "$NATIVE_DIR/merod.pid" ] && kill "$(cat "$NATIVE_DIR/merod.pid")" 2>/dev/null || true
    rm -f "$NATIVE_DIR/merod.pid"
  else
    need_docker
    docker rm -f "$NAME-forward" "$NAME" >/dev/null 2>&1 || true
  fi
  say "stopped (state kept; 'up --fresh' starts over)"
}

cmd_status() {
  if healthy; then
    for p in "${PORTS[@]}"; do echo "  :$p $(curl -fsS -m 2 "http://127.0.0.1:$p/health" || echo down)"; done
  else
    say "not running ($0 up)"
    exit 1
  fi
}

cmd_logs() {
  if native; then tail -n "${LINES:-100}" -f "$NATIVE_DIR/merod.log"; else docker logs -f --tail "${LINES:-100}" "$NAME"; fi
}

# Installs an app on the cluster as a development account whose file passkey is
# kept with the cluster's state, so apps installed once stay visible to the app
# you sign into (a dev bundle is visible to every account on the cluster).
cmd_install() {
  local file="${1:-}"
  [ -n "$file" ] && [ -f "$file" ] || die "usage: $0 install FILE.mpk|FILE.wasm"
  healthy || die "the cluster is not running ($0 up)"
  if native; then
    local ctl=("${MEROCTL_BIN:-meroctl}" --home "$NATIVE_DIR/meroctl")
    "${ctl[@]}" node add dev-cluster http://127.0.0.1:2501/ --allow-mock >/dev/null 2>&1 || true
    "${ctl[@]}" --node dev-cluster login --dev-passkey "$NATIVE_DIR/dev-passkey.json" --name developer >/dev/null
    "${ctl[@]}" --node dev-cluster app install --path "$file"
  else
    local base
    base=$(basename "$file")
    docker exec "$NAME" mkdir -p /data/bundles
    docker cp "$file" "$NAME:/data/bundles/$base" >/dev/null
    docker exec "$NAME" meroctl node add dev-cluster http://127.0.0.1:2501/ --allow-mock >/dev/null 2>&1 || true
    docker exec "$NAME" meroctl --node dev-cluster login --dev-passkey /data/dev-passkey.json --name developer >/dev/null
    docker exec "$NAME" meroctl --node dev-cluster app install --path "/data/bundles/$base"
  fi
}

cmd_android() {
  command -v adb >/dev/null 2>&1 || die "adb not found (Android SDK platform-tools)"
  for p in "${PORTS[@]}"; do adb reverse "tcp:$p" "tcp:$p" >/dev/null; done
  say "adb reverse 2501-2503 done: the phone reaches the cluster at http://localhost:2501"
}

case "${1:-}" in
  up) shift; cmd_up "$@" ;;
  down) cmd_down ;;
  status) cmd_status ;;
  logs) cmd_logs ;;
  urls) urls ;;
  install) shift; cmd_install "$@" ;;
  android) cmd_android ;;
  *) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 2 ;;
esac

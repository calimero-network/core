#!/bin/sh
# Namespace-follow e2e: an admin adds an account at a namespace root, and that
# account's node follows the namespace with no invitation, over real HTTP
# between two real nodes.
#
#   1. node-1 installs kv-store, creates a namespace and a context, writes a key.
#   2. node-1 adds node-2's account (read from node-2's /admin-api/identity).
#   3. node-2 follows with node-1's externalAddrs and reads the key.
#   4. node-1 removes node-2: node-2's next follow is refused (403) and its
#      read fails.
#   5. node-1 re-adds node-2: node-2 follows again, through meroctl this time,
#      and reads the key.
#
# Usage: e2e-namespace-follow.sh [NODE1] [NODE2]
#   Each is a URL, or a merobox node name whose RPC port is read from its
#   config.toml under ./data (merobox's e2e mode picks ports at run time).
#   Defaults: namespace-follow-1 and namespace-follow-2. Both nodes must be
#   fresh, run with --auth-mode embedded, and have an admin account minted at
#   init from MERO_AUTH_ADMIN_USER / MERO_AUTH_ADMIN_PASSWORD, matching
#   MERO_E2E_USER / MERO_E2E_PASS below (see scripts/e2e-auth-seam.sh).
#
#   KV_STORE_MPK names the kv-store bundle; by default the one under dist/
#   (`cargo mero bundle --dev --no-icon` in apps/kv-store, or CI's
#   e2e-app-bundles artifact). MEROCTL names the meroctl binary (default: the
#   one on PATH).

# POSIX sh, not bash: merobox's script step runs /bin/sh (dash on Ubuntu CI)
# whatever the shebang says. No pipefail: every response is captured and
# checked explicitly.
set -eu

# node_url <url or merobox node name>
node_url() {
  case "$1" in
    http://* | https://*)
      echo "$1"
      return
      ;;
  esac
  # Searched, not spelled out: merobox nests the config under ./data at a
  # depth that has changed between releases.
  config=$(find "data/$1" -name config.toml 2>/dev/null | head -1)
  port=""
  if [ -n "$config" ]; then
    port=$(awk '/^\[server\]/{f=1;next} /^\[/{f=0} f' "$config" \
      | grep -o '/tcp/[0-9]*' | head -1 | cut -d/ -f3)
  fi
  [ -n "$port" ] || return 1
  echo "http://127.0.0.1:$port"
}

NODE1_URL=$(node_url "${1:-namespace-follow-1}") || { echo "FAIL no RPC port for ${1:-namespace-follow-1}"; exit 1; }
NODE2_URL=$(node_url "${2:-namespace-follow-2}") || { echo "FAIL no RPC port for ${2:-namespace-follow-2}"; exit 1; }
USERNAME="${MERO_E2E_USER:-dev}"
PASSWORD="${MERO_E2E_PASS:-dev-password}"
MEROCTL="${MEROCTL:-meroctl}"
# Per request. A follow is network-bound (it dials, waits two seconds for the
# topic mesh and asks the peers it reaches), but with one anchor in reach it
# answers in seconds, refusals included; this bounds it generously.
HTTP_TIMEOUT="${HTTP_TIMEOUT:-120}"

KEY="follow-key"
VALUE="written-by-node-1"
KEY2="follow-key-2"
VALUE2="written-by-node-2"
VALUE3="rewritten-by-node-1-after-the-re-add"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

PASS=0

ok() {
  echo "ok   $1"
  PASS=$((PASS + 1))
}

fail() {
  echo "FAIL $1"
  echo "== $PASS passed before the failure =="
  exit 1
}

# api <method> <url> <token> [body]: prints the status; the body lands in
# $WORK/body.
api() {
  method="$1" url="$2" token="$3" body="${4:-}"
  if [ -n "$body" ]; then
    curl -s -o "$WORK/body" -w '%{http_code}' -m "$HTTP_TIMEOUT" -X "$method" \
      -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
      -d "$body" "$url" || true
  else
    curl -s -o "$WORK/body" -w '%{http_code}' -m "$HTTP_TIMEOUT" -X "$method" \
      -H "Authorization: Bearer $token" "$url" || true
  fi
}

body() { cat "$WORK/body" 2>/dev/null || true; }

# expect_200 <label> <status>: the body of a non-200 is printed with the failure.
expect_200() {
  [ "$2" = "200" ] || fail "$1: expected 200, got $2: $(body)"
}

login() { # login <node url>
  login_body=$(jq -n --arg u "$USERNAME" --arg p "$PASSWORD" \
    --argjson ts "$(date +%s)" \
    '{auth_method: "user_password", public_key: $u, client_name: "namespace-follow-e2e",
      permissions: ["admin"], timestamp: $ts,
      provider_data: {username: $u, password: $p}}')
  curl -s -m 10 -X POST "$1/auth/token" -H 'Content-Type: application/json' \
    -d "$login_body" | jq -r '.data.access_token // empty'
}

# read_key <node url> <token> <context id> [key]: the value kv-store holds for
# the key ($KEY by default), or "ERR:<status>:<body>" when the call does not
# succeed.
read_key() {
  rpc=$(jq -n --arg c "$3" --arg k "${4:-$KEY}" \
    '{jsonrpc: "2.0", id: 1, method: "execute",
      params: {contextId: $c, method: "get", argsJson: {key: $k}}}')
  status=$(api POST "$1/jsonrpc" "$2" "$rpc")
  if [ "$status" = "200" ] && [ "$(jq -r 'has("result")' "$WORK/body" 2>/dev/null)" = "true" ]; then
    jq -r '.result.output // "null"' "$WORK/body"
  else
    echo "ERR:$status:$(body)"
  fi
}

# follow <namespace id> <peer addrs json> <application id>: prints the status.
follow() {
  req=$(jq -n --argjson p "$2" --arg a "$3" '{peerAddrs: $p, applicationId: $a}')
  api POST "$NODE2_URL/admin-api/namespaces/$1/follow" "$TOKEN2" "$req"
}

# answered <label> <status>: a follow that does not answer within
# HTTP_TIMEOUT fails the run rather than being retried. Retrying would only
# queue another follow behind the one still running on the node.
answered() {
  [ "$2" != "000" ] || fail "$1: no answer within ${HTTP_TIMEOUT}s"
}

echo "== namespace-follow e2e: node-1 $NODE1_URL, node-2 $NODE2_URL =="

if [ -z "${KV_STORE_MPK:-}" ]; then
  for f in dist/com.calimero.kv-store-*.mpk dist/kv-store*.mpk; do
    [ -f "$f" ] && KV_STORE_MPK="$f" && break
  done
fi
[ -n "${KV_STORE_MPK:-}" ] && [ -f "$KV_STORE_MPK" ] \
  || fail "no kv-store bundle: set KV_STORE_MPK or build it into dist/"
KV_STORE_MPK=$(cd "$(dirname "$KV_STORE_MPK")" && pwd)/$(basename "$KV_STORE_MPK")
# merod reads the path itself: on Windows it needs C:/..., not Git Bash's /c/...
if command -v cygpath >/dev/null 2>&1; then
  KV_STORE_MPK=$(cygpath -m "$KV_STORE_MPK")
fi

TOKEN1=$(login "$NODE1_URL")
[ -n "$TOKEN1" ] || fail "root login on node-1"
TOKEN2=$(login "$NODE2_URL")
[ -n "$TOKEN2" ] || fail "root login on node-2"
ok "root login on both nodes"

# ---- 1. node-1: application, namespace, context, key ----
# Both nodes install the same bundle: an application id is its package and
# signer, so it is the same id on each, and the follow names it.
install=$(jq -n --arg p "$KV_STORE_MPK" '{path: $p}')
status=$(api POST "$NODE1_URL/admin-api/install-dev-application" "$TOKEN1" "$install")
expect_200 "install kv-store on node-1" "$status"
APP_ID=$(jq -r '.data.applicationId // empty' "$WORK/body")
[ -n "$APP_ID" ] || fail "install on node-1 named no applicationId: $(body)"
status=$(api POST "$NODE2_URL/admin-api/install-dev-application" "$TOKEN2" "$install")
expect_200 "install kv-store on node-2" "$status"
APP_ID2=$(jq -r '.data.applicationId // empty' "$WORK/body")
[ "$APP_ID2" = "$APP_ID" ] || fail "node-2 installed $APP_ID2, node-1 $APP_ID"
ok "kv-store installed on both nodes ($APP_ID)"

status=$(api POST "$NODE1_URL/admin-api/namespaces" "$TOKEN1" \
  "$(jq -n --arg a "$APP_ID" '{applicationId: $a, name: "follow-e2e"}')")
expect_200 "create namespace on node-1" "$status"
NS=$(jq -r '.data.namespaceId // empty' "$WORK/body")
[ -n "$NS" ] || fail "create namespace named no namespaceId: $(body)"
ok "namespace created ($NS)"

status=$(api POST "$NODE1_URL/admin-api/contexts" "$TOKEN1" \
  "$(jq -n --arg a "$APP_ID" --arg g "$NS" \
    '{applicationId: $a, contextSeed: null, initializationParams: [], groupId: $g, identitySecret: null}')")
expect_200 "create context on node-1" "$status"
CTX=$(jq -r '.data.contextId // empty' "$WORK/body")
[ -n "$CTX" ] || fail "create context named no contextId: $(body)"
ok "context created ($CTX)"

rpc=$(jq -n --arg c "$CTX" --arg k "$KEY" --arg v "$VALUE" \
  '{jsonrpc: "2.0", id: 1, method: "execute",
    params: {contextId: $c, method: "set", argsJson: {key: $k, value: $v}}}')
status=$(api POST "$NODE1_URL/jsonrpc" "$TOKEN1" "$rpc")
expect_200 "set on node-1" "$status"
[ "$(jq -r 'has("error")' "$WORK/body")" = "false" ] || fail "set on node-1 errored: $(body)"
got=$(read_key "$NODE1_URL" "$TOKEN1" "$CTX")
[ "$got" = "$VALUE" ] || fail "node-1 reads its own key: expected $VALUE, got $got"
ok "node-1 wrote $KEY=$VALUE"

# ---- 2. node-2's account; node-1's addresses ----
status=$(api GET "$NODE2_URL/admin-api/identity" "$TOKEN2")
expect_200 "node-2 identity" "$status"
ACCOUNT2=$(jq -r '.data.accountId // empty' "$WORK/body")
[ -n "$ACCOUNT2" ] || fail "node-2 identity has no accountId: $(body)"
ok "node-2 account is $ACCOUNT2"

# externalAddrs lists what node-1 has confirmed dialable: on the public
# network, after AutoNAT or a relay reservation. Two isolated nodes on one
# machine confirm nothing, so it stays empty there; the follow then gets
# node-1's loopback listen addresses, the same /p2p/-terminated shape, from
# GET /admin-api/network/status.
PEERS="[]"
i=0
while [ $i -lt 10 ]; do
  status=$(api GET "$NODE1_URL/admin-api/identity" "$TOKEN1")
  expect_200 "node-1 identity" "$status"
  [ "$(jq -r '.data.externalAddrs | type' "$WORK/body")" = "array" ] \
    || fail "node-1 /admin-api/identity has no externalAddrs array: $(body)"
  PEERS=$(jq -c '.data.externalAddrs' "$WORK/body")
  [ "$PEERS" != "[]" ] && break
  i=$((i + 1))
  sleep 1
done
if [ "$PEERS" != "[]" ]; then
  ok "node-1 externalAddrs: $PEERS"
else
  ok "node-1 externalAddrs is an array, empty on an isolated pair"
  status=$(api GET "$NODE1_URL/admin-api/network/status" "$TOKEN1")
  expect_200 "node-1 network status" "$status"
  PEERS=$(jq -c '(.data // .) as $s
    | [$s.listenAddrs[] | select(startswith("/ip4/127.0.0.1/")) | . + "/p2p/" + $s.localPeerId]' \
    "$WORK/body")
  [ "$PEERS" != "[]" ] || fail "node-1 listens on no loopback address: $(body)"
  ok "node-1 loopback addresses: $PEERS"
fi

# ---- 3. add, follow, read ----
add_member() {
  status=$(api POST "$NODE1_URL/admin-api/groups/$NS/members" "$TOKEN1" \
    "$(jq -n --arg m "$ACCOUNT2" '{members: [{identity: $m, role: "Member"}]}')")
  expect_200 "node-1 adds node-2's account" "$status"
}

# follow_until_ready <label>: a follow is idempotent, so a key still on its way
# (keyReady false) or a retryable refusal (503) is followed again, bounded.
follow_until_ready() {
  i=0
  while :; do
    status=$(follow "$NS" "$PEERS" "$APP_ID")
    answered "$1" "$status"
    if [ "$status" = "200" ] && [ "$(jq -r '.data.keyReady' "$WORK/body")" = "true" ]; then
      break
    fi
    i=$((i + 1))
    [ $i -lt 20 ] || fail "$1: no ready follow after 20 tries, last $status: $(body)"
    echo "     $1: try $i got $status $(body); retrying"
    sleep 3
  done
  [ "$(jq -r '.data.namespaceId' "$WORK/body")" = "$NS" ] \
    || fail "$1: namespaceId: expected $NS: $(body)"
  [ "$(jq -r '.data.memberAccount' "$WORK/body")" = "$ACCOUNT2" ] \
    || fail "$1: memberAccount: expected $ACCOUNT2: $(body)"
  ok "$1: 200, namespaceId $NS, memberAccount $ACCOUNT2, keyReady true"
}

# set_key <label> <node url> <token> <key> <value>: write through kv-store.
set_key() {
  rpc=$(jq -n --arg c "$CTX" --arg k "$4" --arg v "$5" \
    '{jsonrpc: "2.0", id: 1, method: "execute",
      params: {contextId: $c, method: "set", argsJson: {key: $k, value: $v}}}')
  status=$(api POST "$2/jsonrpc" "$3" "$rpc")
  expect_200 "$1" "$status"
  [ "$(jq -r 'has("error")' "$WORK/body")" = "false" ] || fail "$1 errored: $(body)"
}

# wait_read <label> <node url> <token> <key> <value>: poll a read until it
# holds the value, for a write another node made.
wait_read() {
  i=0
  while :; do
    got=$(read_key "$2" "$3" "$CTX" "$4")
    [ "$got" = "$5" ] && break
    i=$((i + 1))
    [ $i -lt 60 ] || fail "$1: never read $4=$5 in 60s, last: $got"
    sleep 1
  done
  ok "$1: reads $4=$5"
}

# wait_for_value <label>: node-2 joins the context on its own (auto-follow)
# and syncs it; poll the read until it holds node-1's value.
wait_for_value() {
  i=0
  while :; do
    got=$(read_key "$NODE2_URL" "$TOKEN2" "$CTX")
    [ "$got" = "$VALUE" ] && break
    i=$((i + 1))
    [ $i -lt 60 ] || fail "$1: node-2 never read $KEY=$VALUE in 60s, last: $got"
    sleep 1
  done
  ok "$1: node-2 reads $KEY=$VALUE"
}

add_member
ok "node-1 added node-2's account as Member"
follow_until_ready "first follow"

i=0
while :; do
  status=$(api GET "$NODE2_URL/admin-api/contexts" "$TOKEN2")
  expect_200 "node-2 lists contexts" "$status"
  [ "$(jq --arg c "$CTX" '[.data.contexts[] | select(.id == $c)] | length' "$WORK/body")" = "1" ] && break
  i=$((i + 1))
  [ $i -lt 60 ] || fail "node-2 never listed context $CTX in 60s: $(body)"
  sleep 1
done
ok "node-2 lists context $CTX"
wait_for_value "after the first follow"

# node-2 writes and node-1 reads it: node-1 accepts node-2's delta only if it
# resolves node-2's key to its account, which the follower's own device link
# (endorsed by node-1) is what establishes.
set_key "set on node-2" "$NODE2_URL" "$TOKEN2" "$KEY2" "$VALUE2"
wait_read "node-1 after node-2's write" "$NODE1_URL" "$TOKEN1" "$KEY2" "$VALUE2"

# ---- 4. remove: refused, and no read ----
status=$(api POST "$NODE1_URL/admin-api/groups/$NS/members/remove" "$TOKEN1" \
  "$(jq -n --arg m "$ACCOUNT2" '{members: [$m]}')")
expect_200 "node-1 removes node-2's account" "$status"
ok "node-1 removed node-2's account"

# Until the removal reaches node-2, its follow is the completed one (200).
i=0
while :; do
  started=$(date +%s)
  status=$(follow "$NS" "$PEERS" "$APP_ID")
  echo "     follow after removal: $status in $(($(date +%s) - started))s"
  answered "follow after removal" "$status"
  # The named anchor's refusal is final and must say why. A 5xx means the
  # refusal was lost on the way (the regression this leg exists for), so it
  # fails at once rather than being retried. A 200 is node-2 not yet having
  # applied the removal itself, which is worth another try.
  case "$status" in
    403)
      case "$(body)" in
        *'"removed"'* | *": removed"*) break ;;
        *) fail "follow after removal: 403 but not \"removed\": $(body)" ;;
      esac
      ;;
    5*) fail "follow after removal: $status: $(body)" ;;
  esac
  i=$((i + 1))
  [ $i -lt 30 ] || fail "follow after removal: never refused in 30 tries, last $status: $(body)"
  sleep 2
done
ok "follow after removal: 403 ($(jq -r '.error // .message // .' "$WORK/body" 2>/dev/null || body))"

i=0
while :; do
  got=$(read_key "$NODE2_URL" "$TOKEN2" "$CTX")
  case "$got" in ERR:*) break ;; esac
  i=$((i + 1))
  [ $i -lt 30 ] || fail "node-2 still reads after removal (30s): $got"
  sleep 1
done
ok "node-2's read after removal fails: $(echo "${got#ERR:*:}" | jq -c '.error // .' 2>/dev/null || echo "$got")"

# ---- 5. re-add; follow through meroctl; read ----
add_member
ok "node-1 re-added node-2's account"

# meroctl reads its node list and tokens from the user's config directory;
# give it a throwaway one (HOME on macOS, XDG_CONFIG_HOME on Linux).
MEROCTL_HOME="$WORK/meroctl-home"
mkdir -p "$MEROCTL_HOME/.config"
meroctl_n2() {
  HOME="$MEROCTL_HOME" XDG_CONFIG_HOME="$MEROCTL_HOME/.config" \
    NODE2_TOKEN="$TOKEN2" "$MEROCTL" "$@"
}
meroctl_n2 node add follow-e2e-node-2 "$NODE2_URL" --access-token env:NODE2_TOKEN \
  >"$WORK/meroctl-add" 2>&1 || fail "meroctl node add: $(cat "$WORK/meroctl-add")"

peer_flags=$(echo "$PEERS" | jq -r '.[] | "--peer=" + .')
i=0
while :; do
  # One address per line; IFS splits on newlines only, so no address is split.
  old_ifs=$IFS
  IFS='
'
  # shellcheck disable=SC2086 # $peer_flags is one argument per line
  if meroctl_n2 --node follow-e2e-node-2 --output-format json namespace follow "$NS" \
    $peer_flags --application-id "$APP_ID" >"$WORK/body" 2>"$WORK/meroctl-err"; then
    IFS=$old_ifs
    [ "$(jq -r '.data.keyReady' "$WORK/body" 2>/dev/null)" = "true" ] && break
  else
    IFS=$old_ifs
  fi
  i=$((i + 1))
  [ $i -lt 20 ] || fail "meroctl namespace follow: no ready follow after 20 tries: $(body) $(cat "$WORK/meroctl-err")"
  echo "     meroctl follow: try $i: $(body) $(cat "$WORK/meroctl-err"); retrying"
  sleep 3
done
[ "$(jq -r '.data.namespaceId' "$WORK/body")" = "$NS" ] \
  || fail "meroctl follow: namespaceId: expected $NS: $(body)"
[ "$(jq -r '.data.memberAccount' "$WORK/body")" = "$ACCOUNT2" ] \
  || fail "meroctl follow: memberAccount: expected $ACCOUNT2: $(body)"
ok "meroctl namespace follow after re-add: namespaceId $NS, memberAccount $ACCOUNT2, keyReady true"

wait_for_value "after the re-add"

# The removal rotated the namespace key and the re-add delivered the new one:
# a value node-1 writes now is sealed under it, and node-2 must read it.
set_key "set on node-1 after the re-add" "$NODE1_URL" "$TOKEN1" "$KEY" "$VALUE3"
wait_read "node-2 after the re-key" "$NODE2_URL" "$TOKEN2" "$KEY" "$VALUE3"

echo "== $PASS passed, 0 failed =="

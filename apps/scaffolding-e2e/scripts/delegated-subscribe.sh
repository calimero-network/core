#!/bin/sh
#
# A device holding only a key subscribes to a context and observes a write land
# on it — over BOTH event transports.
#
# `delegated-session.yml` proves the SESSION: a keyholder logs in to a node it
# does not own, reads, and writes. What no scenario covers, at any layer, is
# what that session can OBSERVE. Of the account/delegated workflows beside this
# one, none mentions websocket, `/ws`, `text/event-stream` or subscribe; the
# only matches for "subscribe" anywhere in `workflows/` are prose about
# gossipsub subscriber sets, which is a different thing entirely.
#
# For a CRDT system that is the operation applications depend on most. A client
# that can write but never observe convergence is not usable: it shows a UI that
# looks frozen while the state underneath it moves.
#
# Both transports, because they authenticate by DIFFERENT means — WebSocket
# takes the token in the query string (a browser cannot set a header on a
# WebSocket), SSE takes it in an `Authorization` header. One working proves
# nothing about the other, and each is a separate route in the permission
# validator.
#
# Usage: delegated-subscribe.sh <node> <context_id> <device_secret> <credential> <node_key>
#
# The session-minting half is `delegated-session.sh`'s, deliberately unchanged:
# same challenge, same `merod account login-statement`, same `/auth/token` post.
# This scenario differs only in what it does with the session it gets, so
# re-deriving that half here would be a second copy to keep in step.

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
CONTEXT="$2"
DEVICE_SECRET="$3"
CREDENTIAL="$4"
NODE_KEY="$5"

[ -n "${NODE}" ] || fail "usage: delegated-subscribe.sh <node> <context> <device_secret> <credential> <node_key>"
[ -n "${CONTEXT}" ] || fail "no context id given"
[ -n "${DEVICE_SECRET}" ] || fail "no device secret given"
[ -n "${CREDENTIAL}" ] || fail "no device credential given"
[ -n "${NODE_KEY}" ] || fail "no node key given"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

# --- 1. The node issues a challenge -----------------------------------------
#
# Fetched immediately before signing: it is single-use and short-lived.

CHALLENGE=$(curl -fsS "${URL}/auth/challenge" \
    | sed -n 's/.*"challenge"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)".*/\1/p')
[ -n "${CHALLENGE}" ] || fail "the node issued no challenge"
echo "challenge: ${CHALLENGE}"

# --- 2. The device signs a login statement, offline -------------------------
#
# `merod account login-statement` rather than a re-encoding here: the borsh
# layout and signing domain live in one place, and a harness carrying its own
# copy fails in the worst direction — the copy passes its own checks while every
# real client is refused.

SIGNED=$(offline_merod account login-statement \
    --challenge "${CHALLENGE}" \
    --node "${NODE_KEY}" \
    --device-secret "${DEVICE_SECRET}" \
    --generate-session-key \
    --credential "${CREDENTIAL}" \
    --audience cli)

STATEMENT=$(echo "${SIGNED}" | head -1)
SESSION_KEY=$(echo "${SIGNED}" | sed -n 's/^Session:[[:space:]]*//p')
[ -n "${STATEMENT}" ] || fail "merod produced no login statement"
[ -n "${SESSION_KEY}" ] || fail "merod produced no session key"

# --- 3. The node mints a session from it ------------------------------------
#
# `permissions` is OMITTED, not set: leaving it unset takes the provider's own
# `session_permissions`, which is where `context:subscribe` comes from
# (`AccountProofConfig::default`). Asking for a scope here would test what this
# script requested rather than what an operator's node actually grants — and
# the grant is the thing under test, since `/ws` and `/sse` are mapped to
# `context:subscribe` in the permission validator.

TOKEN_BODY=$(printf '{"auth_method":"account_proof","public_key":"%s","client_name":"%s","timestamp":%s,"provider_data":{"challenge":"%s","login_statement":"%s","account_proof":"%s"}}' \
    "${SESSION_KEY}" "${URL}" "$(date +%s)" "${CHALLENGE}" "${STATEMENT}" "${CREDENTIAL}")

TOKEN_RES=$(curl -sS -X POST "${URL}/auth/token" \
    -H 'Content-Type: application/json' \
    -d "${TOKEN_BODY}")
TOKEN=$(echo "${TOKEN_RES}" | sed -n 's/.*"access_token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -n "${TOKEN}" ] || fail "no session was minted: ${TOKEN_RES}"
echo "session minted with no password"

# --- 4. That session subscribes, on each transport in turn ------------------
#
# Python, unlike every other script here, because neither transport is reachable
# from curl: WebSocket needs an HTTP upgrade and RFC 6455 framing, SSE needs a
# connection held open while a second request subscribes on it. The probe is
# stdlib-only for the same reason these scripts use curl at all — the runner is
# whatever it is, and nothing may need installing.
#
# A DISTINCT key per transport. Sharing one would let a single delivered event
# satisfy both runs, and worse, the second run would find the value already set
# — so a node that delivered nothing the second time would still look right to
# anyone reading the final state.
#
# The write is made on the SAME node the subscription is on. What is under test
# is event delivery to a device-key session, not replication: routing a write
# through the peer would make this scenario fail whenever sync was slow, and
# name subscription as the cause. `wait_for_sync` in the scenario around this
# covers propagation separately, and on its own barrier.

PROBE="$(dirname "$0")/subscribe-probe.py"

for TRANSPORT in ws sse; do
    echo "--- ${TRANSPORT} ---"
    python3 "${PROBE}" \
        --transport "${TRANSPORT}" \
        --url "${URL}" \
        --token "${TOKEN}" \
        --context "${CONTEXT}" \
        --write-url "${URL}" \
        --key "watched-${TRANSPORT}" \
        --value "seen-over-${TRANSPORT}" \
        || fail "the ${TRANSPORT} subscription did not deliver the write"
done

echo "PASS: a device-key session subscribed over WebSocket AND SSE, and observed a write on each"

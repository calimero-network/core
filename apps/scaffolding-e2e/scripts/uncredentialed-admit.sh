#!/bin/sh
#
# A joiner holding only an invitation reaches direct admission on a node with
# embedded auth, carrying no credential of any kind.
#
# `POST /admin-api/namespaces/{id}/admit` is the one route a keyholder with no
# node calls on somebody else's node, and every SDK calls it with no
# `Authorization` header and no proof: the joiner's signature over the op is the
# authorization. It used to sit behind the embedded auth guard, so on such a node
# the joiner was answered `401` before its op was read.
#
# What this asserts, against a running node:
#
#   1. The route is reached without a credential: the answers come from the
#      handler, never `401` from the guard.
#   2. Reached is not accepted. An op that is not a signed join is refused `400`,
#      and an invitation naming no admitter `403`, before anything is published.
#   3. Only this route left the guard: `/join` beside it still answers `401`.
#
# A SUCCESSFUL admission needs a join op signed by a device key, which nothing in
# core produces offline (mero-js's `signMemberJoinOp` does, and its
# `join-with-node` e2e drives that half). Every refusal here is decided after the
# point the guard used to stop the request, so each one is proof the route is
# open, and the handler's success path is covered by its unit tests.
#
# Usage: uncredentialed-admit.sh <node> <namespace_id>
#
# The invitation is minted by the node's operator (it is the namespace admin, so
# the default `admitters` names it). Only that step carries a token.

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
NAMESPACE="$2"

[ -n "${NODE}" ] || fail "usage: uncredentialed-admit.sh <node> <namespace_id>"
[ -n "${NAMESPACE}" ] || fail "no namespace id given"
command -v jq >/dev/null || fail "jq is required to carry the invitation verbatim"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

# --- The operator mints an invitation (the one credentialed call) ------------

LOGIN=$(curl -sS -X POST "${URL}/auth/token" \
    -H 'Content-Type: application/json' \
    -d "$(printf '{"auth_method":"user_password","public_key":"uncredentialed-admit","client_name":"%s","timestamp":%s,"provider_data":{"username":"%s","password":"%s"}}' \
        "${URL}" "$(date +%s)" "${MERO_AUTH_ADMIN_USER:-dev}" "${MERO_AUTH_ADMIN_PASSWORD:-dev-password}")")
TOKEN=$(echo "${LOGIN}" | jq -r '.data.access_token // .access_token // empty')
[ -n "${TOKEN}" ] || fail "the operator could not log in: ${LOGIN}"

INVITE=$(curl -sS -X POST "${URL}/admin-api/namespaces/${NAMESPACE}/invite" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{}')
INVITATION=$(echo "${INVITE}" | jq -c '.data.invitation // empty')
[ -n "${INVITATION}" ] || fail "no invitation was minted: ${INVITE}"
echo "invitation minted"

# From here on nothing carries a token, a cookie or a proof.

# POST <path> <body>, uncredentialed; prints "<status> <body>".
post_bare() {
    _out=$(mktemp)
    _status=$(curl -sS -o "${_out}" -w '%{http_code}' -X POST "${URL}$1" \
        -H 'Content-Type: application/json' -d "$2")
    echo "${_status} $(cat "${_out}")"
    rm -f "${_out}"
}

ADMIT="/admin-api/namespaces/${NAMESPACE}/admit"

# --- 1+2. Reached, and an op that is not a signed join is refused ------------
#
# `400 ... not a SignedNamespaceOp` is decided after the handler found this node
# in the invitation's admitters, so it also proves the invitation was read.

RES=$(post_bare "${ADMIT}" "$(jq -nc --argjson inv "${INVITATION}" '{invitation: $inv, signedOp: "00"}')")
case "${RES}" in
    401*) fail "an uncredentialed admit was refused by the auth guard: ${RES}" ;;
    "400 "*"not a SignedNamespaceOp"*) echo "reached without a credential, garbage refused: ${RES}" ;;
    *) fail "expected 400 for an op that is not a signed join, got: ${RES}" ;;
esac

RES=$(post_bare "${ADMIT}" "$(jq -nc --argjson inv "${INVITATION}" '{invitation: $inv, signedOp: "zz"}')")
case "${RES}" in
    "400 "*"not hex"*) echo "non-hex op refused: ${RES}" ;;
    *) fail "expected 400 for a non-hex op, got: ${RES}" ;;
esac

# --- 2. An invitation naming no admitter is refused before the op is decoded --

RES=$(post_bare "${ADMIT}" "$(jq -nc --argjson inv "${INVITATION}" '{invitation: ($inv | .invitation.admitters = []), signedOp: "00"}')")
case "${RES}" in
    "403 "*) echo "invitation naming no admitter refused: ${RES}" ;;
    *) fail "expected 403 for an invitation naming no admitter, got: ${RES}" ;;
esac

# --- 3. The guard still covers the rest of the namespace routes ---------------

RES=$(post_bare "/admin-api/namespaces/${NAMESPACE}/join" '{}')
case "${RES}" in
    401*) echo "join still needs a credential" ;;
    *) fail "an uncredentialed /join was not refused by the guard: ${RES}" ;;
esac

echo "PASS: direct admission is reachable without a credential and refuses what does not verify"

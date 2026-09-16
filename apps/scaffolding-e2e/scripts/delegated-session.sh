#!/bin/sh
#
# A device that holds no node obtains a session and reads a context, with no
# password anywhere in the flow.
#
# This is the half of epic #3928 that `delegated-authorship.yml` does not cover.
# That scenario proves a keyholder can WRITE through a relay, but it logs in with
# `auth_mode: embedded` and a username and password, which is precisely the
# provider the target architecture deletes. What has never been exercised
# end to end is the flow the epic states as its own criterion:
#
#   A client holding only an account root and a device key can: obtain a session
#   against a node it is not the owner of, read a context it is a member of, and
#   submit a delegated write — with no password anywhere in the flow.
#
# Usage: delegated-session.sh <node> <context_id> <device_secret> <credential> <node_key>
#
# <node_key> is the node's own public key, captured by the scenario's
# `node_identity` step. The statement is signed over it, and the verifier checks
# it (`LoginStatement::addressed_to`): without that binding a hostile relay could
# fetch a challenge here, serve it to a user as its own, and replay what it got
# back. Passed in rather than read from an endpoint so this script asserts on the
# value the scenario already pinned.
#
# Uses curl against the node rather than meroctl, as account-api.sh does and for
# the same reason: the merod image ships no CLI, so a `target: local` script has
# none to call.

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
CONTEXT="$2"
DEVICE_SECRET="$3"
CREDENTIAL="$4"
NODE_KEY="$5"

[ -n "${NODE}" ] || fail "usage: delegated-session.sh <node> <context> <device_secret> <credential>"
[ -n "${CONTEXT}" ] || fail "no context id given"
[ -n "${DEVICE_SECRET}" ] || fail "no device secret given"
[ -n "${CREDENTIAL}" ] || fail "no device credential given"
[ -n "${NODE_KEY}" ] || fail "no node key given"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

# --- 1. The node issues a challenge -----------------------------------------
#
# Fetched immediately before signing: it is single-use and short-lived, so a
# statement minted against a stale one is refused before any signature work.

CHALLENGE=$(curl -fsS "${URL}/auth/challenge" \
    | sed -n 's/.*"challenge"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)".*/\1/p')
[ -n "${CHALLENGE}" ] || fail "the node issued no challenge"
echo "challenge: ${CHALLENGE}"

# --- 2. The device signs a login statement, offline -------------------------
#
# `merod account login-statement` rather than a re-encoding here. The borsh
# layout and the signing domain live in one place on purpose —
# `LoginStatement::signing_payload` says so in its own docs — and a harness
# carrying its own copy fails in the worst direction: the copy passes its own
# checks while every real client is refused.
#
# The session key is ephemeral and distinct from the device key: it is what the
# token authorises, so a leaked session cannot be escalated into use of the
# device key itself.
# ONE invocation: each call signs a fresh statement with its own timestamps, so
# taking the statement from one and the keys from another would post a statement
# naming a session key it never signed over — refused, and confusingly so.
# `--generate-session-key` mints the pair here precisely so this script does no
# crypto of its own.
SIGNED=$(offline_merod "${NODE}" account login-statement \
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
echo "statement signed, session key ${SESSION_KEY}"

# --- 3. The node mints a session from it ------------------------------------

# `timestamp` is REQUIRED and `BaseTokenRequest` is `deny_unknown_fields`, so a
# body missing it is rejected before any provider runs -- and the refusal names
# deserialization, not the login, which reads as though the statement were at
# fault. `permissions` is deliberately OMITTED rather than set: leaving it unset
# takes the provider's own `session_permissions` (`context:intent`,
# `context:query`, `context:subscribe`) instead of asking for authority a
# delegated session must not have. mero-js#84 is the same mistake made the other
# way -- `authenticate()` hardcodes `['admin']`.
TOKEN_BODY=$(printf '{"auth_method":"account_proof","public_key":"%s","client_name":"%s","timestamp":%s,"provider_data":{"challenge":"%s","login_statement":"%s","account_proof":"%s"}}' \
    "${SESSION_KEY}" "${URL}" "$(date +%s)" "${CHALLENGE}" "${STATEMENT}" "${CREDENTIAL}")

TOKEN_RES=$(curl -sS -X POST "${URL}/auth/token" \
    -H 'Content-Type: application/json' \
    -d "${TOKEN_BODY}")
TOKEN=$(echo "${TOKEN_RES}" | sed -n 's/.*"access_token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -n "${TOKEN}" ] || fail "no session was minted, and this is the criterion: ${TOKEN_RES}"
echo "session minted with no password"

# --- 4. The session reads a context it is a member of ------------------------
#
# #3931. A session authorises READS only, so this is the whole of what a
# delegated client can do without minting a warrant.

READ_RES=$(curl -sS -X POST "${URL}/admin-api/contexts/${CONTEXT}/query" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{"method":"get","argsJson":{"key":"delegated"}}')
# Assert the VALUE, not merely that some field came back. The response shape
# is `QueryContextApiResponseData { returns }` (crates/server/.../query_context.rs),
# and an earlier version of this check grepped for `"output"` -- a field that
# does not exist on this route. It therefore failed while the read was
# succeeding, and would equally have passed on any body that happened to carry
# the word. `read-by-a-keyholder` is what the scenario wrote two steps up, so
# matching it proves the keyholder read THIS context's state rather than an
# empty or defaulted answer.
echo "${READ_RES}" | grep -q '"returns"[[:space:]]*:[[:space:]]*"read-by-a-keyholder"' \
    || fail "the delegated read did not return the value the scenario wrote: ${READ_RES}"
echo "delegated read served: ${READ_RES}"

# --- 5. Events are deliberately NOT asserted here ---------------------------
#
# A subscribed session only sees an event if something WRITES while it is
# listening, and merobox runs steps sequentially: during any sleep in this
# script nothing else is running, so a stream assertion here could only ever
# time out. An earlier draft slept 8s waiting for traffic that no step produced.
#
# The honest trigger is a delegated WRITE from this same session -- a warrant
# minted by the device, spent by the relay -- which is #3942's real shape and
# needs the authorship grant `delegated-authorship.yml` sets up. Combining the
# two is worth doing and is not this scenario's first job: what has never run
# end to end is the password-free SESSION, and that is what the steps above
# prove.

echo "PASS: a device holding only a key obtained a session and read a context, with no password in the flow"

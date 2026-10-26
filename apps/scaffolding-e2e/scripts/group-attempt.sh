#!/bin/sh
#
# Send one admin-API request a node may refuse and report the status it answered.
# Exits 0 whatever the status, so a scenario can then read the state to judge the outcome;
# it fails only when the node cannot be reached or logged into, which must never read as a refusal.
#
# Usage: group-attempt.sh <node> <method> <path> <json_body>

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
METHOD="$2"
REQUEST_PATH="$3"
BODY="$4"

[ -n "${NODE}" ] && [ -n "${METHOD}" ] && [ -n "${REQUEST_PATH}" ] && [ -n "${BODY}" ] \
    || fail "usage: group-attempt.sh <node> <method> <path> <json_body>"
command -v jq >/dev/null || fail "jq is required to read the login"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"
TOKEN=$(operator_token "${URL}" group-attempt) || exit 1

STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -X "${METHOD}" "${URL}${REQUEST_PATH}" \
    -H "Authorization: Bearer ${TOKEN}" -H 'Content-Type: application/json' -d "${BODY}")
[ "${STATUS}" != 000 ] || fail "${NODE} did not answer ${METHOD} ${REQUEST_PATH}"
echo "${METHOD} ${REQUEST_PATH} answered ${STATUS}"

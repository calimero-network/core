#!/bin/sh
# POST the admin abort-migration route from INSIDE the node container
# (PR-6d task 6d.6 scenario 31). merobox runs this via a `script` step with
# `target: nodes`, so `$1` is the resolved namespace id (passed through the
# step's `args`). The node's admin API listens on 127.0.0.1:2528/admin-api
# inside the container (DEFAULT_RPC_PORT); the abort handler admin-gates the
# call against the node's own identity, which is the namespace admin in this
# single-node scenario.
#
# The load-bearing assertion is the node log line the abort handler emits
# ("migration logically aborted: target flipped back ..." from
# crates/context/src/handlers/abort_migration.rs) — this script only has to
# get the request to the handler. We therefore print the HTTP status/body for
# diagnostics and only hard-fail if the request could not be issued at all.

set -u

NAMESPACE_ID="${1:?namespace id arg required}"
ADMIN_BASE="http://127.0.0.1:2528/admin-api"
URL="${ADMIN_BASE}/groups/${NAMESPACE_ID}/migration/abort"

echo "POST ${URL}"

if ! command -v curl >/dev/null 2>&1; then
    echo "curl not found in node image" >&2
    exit 1
fi

# -s silent, -S show errors, -w status line, -X POST with empty body.
HTTP_STATUS=$(curl -sS -o /tmp/abort-resp.json -w '%{http_code}' \
    -X POST -H 'Content-Type: application/json' "${URL}")
CURL_RC=$?

echo "http_status=${HTTP_STATUS}"
if [ -f /tmp/abort-resp.json ]; then
    echo "response_body=$(cat /tmp/abort-resp.json)"
fi

if [ "${CURL_RC}" -ne 0 ]; then
    echo "curl failed to reach the abort route (rc=${CURL_RC})" >&2
    exit "${CURL_RC}"
fi

# A 2xx means the abort RPC ran (idempotent — aborted may be true or false).
case "${HTTP_STATUS}" in
    2*) echo "abort route reached and accepted"; exit 0 ;;
    *)  echo "abort route returned non-2xx status ${HTTP_STATUS}" >&2; exit 1 ;;
esac

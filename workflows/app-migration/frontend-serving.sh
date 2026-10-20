#!/bin/sh
#
# Check what a node serves on an app's own host for two releases of one app.
# Usage: frontend-serving.sh <node> <app_id> <release_a> <release_b>
# `release_b` is the newest install, so it is where `/` must redirect.
# curl rather than meroctl: the merod image ships no CLI for a `target: local` script.

. apps/scaffolding-e2e/scripts/account-api.sh

node="${1:?node}"
app_id="${2:?application id}"
release_a="${3:?release a}"
release_b="${4:?release b}"
fixtures=apps/migrations/frontend-release

url=$(node_url "${node}") || fail "no admin URL for ${node}"
port="${url##*:}"

# The host label is the 32-byte application id (hex here) as unpadded lowercase base32.
label=$(python3 -I -c 'import base64, sys; print(base64.b32encode(bytes.fromhex(sys.argv[1])).decode().rstrip("=").lower())' "${app_id}") \
    || fail "could not derive the host label from ${app_id}"
app_host="${label}.localhost:${port}"

headers=$(mktemp)
body=$(mktemp)
trap 'rm -f "${headers}" "${body}"' EXIT

# Fetch `path` on `host`, leaving the status in ${status} and the response in the temp files.
fetch() {
    status=$(curl -sS -D "${headers}" -o "${body}" -w '%{http_code}' -H "Host: $1" "${url}$2") \
        || fail "request for $2 on $1 failed"
}

want_status() {
    fetch "$1" "$2"
    [ "${status}" = "$3" ] || fail "$1 $2 answered ${status}, wanted $3"
}

want_header() {
    grep -qi "^$1" "${headers}" || fail "$2 has no $1 header: $(cat "${headers}")"
}

check_release() {
    version="$1"
    want_status "${app_host}" "/${version}/" 200
    cmp -s "${body}" "${fixtures}/v${version%%.*}/index.html" \
        || fail "/${version}/ did not serve release ${version}'s index.html"
    grep -q "frontend-release-${version}" "${body}" || fail "/${version}/ does not carry its version string"
    want_header 'content-security-policy:' "/${version}/"
    want_header 'x-content-type-options: nosniff' "/${version}/"
    want_header 'etag:' "/${version}/"
    want_header 'content-length:' "/${version}/"
    want_status "${app_host}" "/${version}/app.js" 200
    cmp -s "${body}" "${fixtures}/v${version%%.*}/app.js" || fail "/${version}/app.js is not release ${version}'s file"
}

check_release "${release_a}"
check_release "${release_b}"

want_status "${app_host}" / 302
grep -qi "^location: /${release_b}/" "${headers}" || fail "/ did not redirect to ${release_b}: $(cat "${headers}")"

# An app host serves app files and nothing else: no dashboard, admin, auth or RPC.
for path in /admin-dashboard /admin-api/applications /auth/login /jsonrpc; do
    want_status "${app_host}" "${path}" 404
done

# The node's own host is untouched.
fetch "localhost:${port}" /admin-dashboard/
case "${status}" in
    200 | 301 | 302 | 307 | 308) ;;
    *) fail "the normal host answered ${status} for /admin-dashboard/" ;;
esac

# An app's page is third-party code: its origin gets 403 on the node's own routes
# before auth is consulted, while the same request without an Origin reaches auth and gets 401.
status=$(curl -sS -o /dev/null -w '%{http_code}' -H "Origin: http://${app_host}" "${url}/admin-api/applications") \
    || fail "request for /admin-api/applications from the app origin failed"
[ "${status}" = 403 ] || fail "/admin-api/applications from http://${app_host} answered ${status}, wanted 403"
status=$(curl -sS -o /dev/null -w '%{http_code}' "${url}/admin-api/applications") \
    || fail "request for /admin-api/applications without an Origin failed"
[ "${status}" = 401 ] || fail "/admin-api/applications without an Origin answered ${status}, wanted 401"

echo "OK: ${app_host} serves releases ${release_a} and ${release_b}"

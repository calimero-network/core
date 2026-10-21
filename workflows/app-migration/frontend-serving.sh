#!/bin/sh
#
# Check what a node serves on an app's own host for two releases of one app.
# Usage: frontend-serving.sh <node> <app_id> <release_a> <release_b> <embedded|proxy> [context_id]
# `release_b` is the newest install, so it is where `/` must redirect.
# `proxy` is a node that enforces no token, so only its refusals are checked.
# curl rather than meroctl: the merod image ships no CLI for a `target: local` script.

. apps/scaffolding-e2e/scripts/account-api.sh

node="${1:?node}"
app_id="${2:?application id}"
release_a="${3:?release a}"
release_b="${4:?release b}"
mode="${5:?mode}"
context_id="${6:-}"
fixtures=apps/migrations/frontend-release

url=$(node_url "${node}") || fail "no admin URL for ${node}"
port="${url##*:}"
node_host="localhost:${port}"

# The host label is the 32-byte application id (hex here) as unpadded lowercase base32.
label=$(python3 -I -c 'import base64, sys; print(base64.b32encode(bytes.fromhex(sys.argv[1])).decode().rstrip("=").lower())' "${app_id}") \
    || fail "could not derive the host label from ${app_id}"
app_host="${label}.localhost:${port}"

headers=$(mktemp)
body=$(mktemp)
trap 'rm -f "${headers}" "${body}"' EXIT

# Fetch `path` on `host`, leaving the status in ${status} and the response in the temp files.
# Further arguments go to curl.
fetch() {
    _host="$1"
    _path="$2"
    shift 2
    status=$(curl -sS -D "${headers}" -o "${body}" -w '%{http_code}' -H "Host: ${_host}" "$@" "${url}${_path}") \
        || fail "request for ${_path} on ${_host} failed"
}

# want_status <host> <path> <status> [curl args]
want_status() {
    _h="$1"
    _p="$2"
    _want="$3"
    shift 3
    fetch "${_h}" "${_p}" "$@"
    [ "${status}" = "${_want}" ] || fail "${_h} ${_p} answered ${status}, wanted ${_want}: $*"
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

# A root login, then its access token on stdout.
admin_token() {
    _res=$(curl -sS -X POST "${url}/auth/token" -H 'Content-Type: application/json' \
        -d "$(printf '{"auth_method":"user_password","public_key":"%s","client_name":"frontend-serving","permissions":["admin"],"timestamp":%s,"provider_data":{"username":"%s","password":"%s"}}' \
            "${MERO_AUTH_ADMIN_USER}" "$(date +%s)" "${MERO_AUTH_ADMIN_USER}" "${MERO_AUTH_ADMIN_PASSWORD}")")
    _token=$(echo "${_res}" | sed -n 's/.*"access_token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    [ -n "${_token}" ] || fail "no admin token: ${_res}"
    echo "${_token}"
}

rpc_body() {
    printf '{"jsonrpc":"2.0","id":1,"method":"execute","params":{"contextId":"%s","method":"schema_info","argsJson":{}}}' "${context_id}"
}

check_embedded() {
    [ -n "${context_id}" ] || fail "embedded mode needs a context id"
    check_release "${release_a}"
    check_release "${release_b}"

    want_status "${app_host}" / 302
    grep -qi "^location: /${release_b}/" "${headers}" || fail "/ did not redirect to ${release_b}: $(cat "${headers}")"

    # A missing file is still an app-host response, so it carries the app headers.
    want_status "${app_host}" "/${release_b}/missing.js" 404
    want_header 'content-security-policy:' "a missing file"
    want_header 'x-content-type-options: nosniff' "a missing file"

    # No dashboard, admin API or login on an app host.
    for path in /admin-dashboard /admin-api/applications /auth/login; do
        want_status "${app_host}" "${path}" 404
    done

    # The node's own host is untouched.
    fetch "${node_host}" /admin-dashboard/
    case "${status}" in
        200 | 301 | 302 | 307 | 308) ;;
        *) fail "the normal host answered ${status} for /admin-dashboard/" ;;
    esac

    # The app API on the app host reaches the node's auth, and only from its own origin.
    token=$(admin_token)
    json='Content-Type: application/json'
    want_status "${app_host}" /jsonrpc 401 -X POST -H "${json}" -d "$(rpc_body)"
    want_status "${app_host}" /jsonrpc 200 -X POST -H "${json}" -H "Authorization: Bearer ${token}" \
        -H "Origin: http://${app_host}" -d "$(rpc_body)"
    grep -q '"result"' "${body}" || fail "the app-host call returned no result: $(cat "${body}")"
    for origin in "http://evil.localhost:${port}" "http://other.localhost:${port}"; do
        want_status "${app_host}" /jsonrpc 403 -X POST -H "${json}" -H "Authorization: Bearer ${token}" \
            -H "Origin: ${origin}" -d "$(rpc_body)"
    done

    # The same app origin on the node's own host is refused before auth, and reaches auth without it.
    want_status "${node_host}" /jsonrpc 403 -X POST -H "${json}" -H "Origin: http://${app_host}" -d "$(rpc_body)"
    want_status "${node_host}" /jsonrpc 401 -X POST -H "${json}" -d "$(rpc_body)"
    want_status "${node_host}" /admin-api/applications 401
    want_status "${node_host}" /admin-api/applications 403 -H "Origin: http://${app_host}"
    # A loopback dev page is not an app origin: auth answers it, the origin guard does not.
    want_status "${node_host}" /admin-api/applications 401 -H 'Origin: http://localhost:5173'
}

check_proxy() {
    want_status "${app_host}" /jsonrpc 403 -X POST -H 'Content-Type: application/json' -d '{}'
    want_header 'content-type: text/plain' "/jsonrpc without a token"
    [ -s "${body}" ] || fail "the refusal has no body"
    want_status "${node_host}" /admin-api/applications 403 -H "Origin: http://${app_host}"
    fetch "${node_host}" /admin-api/applications -H 'Origin: http://localhost:5173'
    [ "${status}" != 403 ] || fail "a loopback dev page was refused as an app origin"
}

case "${mode}" in
    embedded) check_embedded ;;
    proxy) check_proxy ;;
    *) fail "unknown mode ${mode}" ;;
esac

echo "OK (${mode}): ${app_host}"

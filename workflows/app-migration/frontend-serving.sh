#!/bin/sh
#
# Check what a node serves on an app's own host for two releases of one app.
# Usage: frontend-serving.sh <node> <app_id> <release_a> <release_b> <embedded|proxy> [context_id] [apps_port]
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
apps_port="${7:-}"
app_permissions='"application:list","context:list","context:create","context:execute","namespace","group"'
fixtures=apps/migrations/frontend-release
refused=403 # the answer to a route the node does not give an app origin

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

# token_field <field> <json>: a string field of a token response, on stdout.
token_field() {
    _value=$(echo "$2" | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p")
    [ -n "${_value}" ] || fail "no $1 in: $2"
    echo "${_value}"
}

# A root login, as the token response on stdout.
admin_login() {
    _login=$(printf '{"auth_method":"user_password","public_key":"%s","client_name":"frontend-serving","permissions":["admin"],"timestamp":%s,"provider_data":{"username":"%s","password":"%s"}}' \
        "${MERO_AUTH_ADMIN_USER}" "$(date +%s)" "${MERO_AUTH_ADMIN_USER}" "${MERO_AUTH_ADMIN_PASSWORD}")
    curl -sS -X POST "${url}/auth/token" -H 'Content-Type: application/json' -d "${_login}"
}

# A client key token with no binding, as the login page mints for an app, on stdout.
app_token() {
    _key=$(printf '{"context_id":"","context_identity":"","permissions":[%s]}' "${app_permissions}")
    _res=$(curl -sS -X POST "${url}/admin/client-key" -H 'Content-Type: application/json' \
        -H "Authorization: Bearer $1" -d "${_key}")
    token_field access_token "${_res}"
}

rpc_body() {
    printf '{"jsonrpc":"2.0","id":1,"method":"execute","params":{"contextId":"%s","method":"schema_info","argsJson":{}}}' "${context_id}"
}

# rpc <host> <status> [curl args]: a JSON-RPC call that must answer <status>.
rpc() {
    _rh="$1"
    _rs="$2"
    shift 2
    want_status "${_rh}" /jsonrpc "${_rs}" -X POST -H 'Content-Type: application/json' -d "$(rpc_body)" "$@"
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

    # No dashboard on an app host, no admin route outside the app's allowlist, and login is the node's own.
    want_status "${app_host}" /admin-dashboard 404
    for path in /admin-api/blobs /admin-api/alias/list/context "/admin-api/applications/${app_id}/abi"; do
        want_status "${app_host}" "${path}" "${refused}"
        [ -s "${body}" ] || fail "${path} was refused without a reason"
    done
    want_status "${app_host}" "/auth/login?app-id=x&callback-url=y" 302
    grep -qi "^location: //localhost:${port}/auth/login?app-id=x&callback-url=y" "${headers}" \
        || fail "the app host's login did not redirect to the node's own origin: $(cat "${headers}")"

    # The node's own host is untouched.
    fetch "${node_host}" /admin-dashboard/
    case "${status}" in
        200 | 301 | 302 | 307 | 308) ;;
        *) fail "the normal host answered ${status} for /admin-dashboard/" ;;
    esac

    # The app API on the app host reaches the node's auth, with a token that is not an admin's, and only from its own origin.
    login=$(admin_login)
    admin=$(token_field access_token "${login}")
    admin_refresh=$(token_field refresh_token "${login}")
    token=$(app_token "${admin}")
    rpc "${app_host}" 401
    rpc "${app_host}" 200 -H "Authorization: Bearer ${token}" -H "Origin: http://${app_host}"
    grep -q '"result"' "${body}" || fail "the app-host call returned no result: $(cat "${body}")"
    rpc "${app_host}" "${refused}" -H "Authorization: Bearer ${admin}" -H "Origin: http://${app_host}"
    for origin in "http://evil.localhost:${port}" "http://other.localhost:${port}"; do
        rpc "${app_host}" "${refused}" -H "Authorization: Bearer ${token}" -H "Origin: ${origin}"
    done

    # The admin routes an app needs answer it, bound to its app; the rest of the admin API does not.
    want_status "${app_host}" /admin-api/contexts 401
    want_status "${app_host}" /admin-api/contexts 200 -H "Authorization: Bearer ${token}"
    grep -q "${context_id}" "${body}" || fail "the app's own context is not listed: $(cat "${body}")"
    for path in /admin-api/applications "/admin-api/applications/${app_id}" "/admin-api/namespaces/for-application/${app_id}"; do
        want_status "${app_host}" "${path}" 200 -H "Authorization: Bearer ${token}"
        grep -q "${app_id}" "${body}" || fail "${path} did not name the app: $(cat "${body}")"
    done
    # An admin session is not renewed on an app host, whatever the page passes.
    want_status "${app_host}" /auth/refresh "${refused}" -X POST -H 'Content-Type: application/json' -H "Origin: http://${app_host}" \
        -d "$(printf '{"access_token":"%s","refresh_token":"%s"}' "${admin}" "${admin_refresh}")"
    want_status "${app_host}" /admin-api/contexts "${refused}" -H "Authorization: Bearer ${admin}"
    want_status "${app_host}" "/admin-api/contexts/${context_id}" "${refused}" -X DELETE -H "Authorization: Bearer ${token}"
    want_status "${app_host}" "/admin-api/contexts/${context_id}" "${refused}" -X DELETE -H "Authorization: Bearer ${admin}"
    want_status "${node_host}" /admin-api/contexts 200 -H "Authorization: Bearer ${admin}"
    grep -q "${context_id}" "${body}" || fail "the node's own host no longer lists the context: $(cat "${body}")"

    # The same app origin on the node's own host is refused before auth, and reaches auth without it.
    rpc "${node_host}" "${refused}" -H "Origin: http://${app_host}"
    rpc "${node_host}" 401
    want_status "${node_host}" /admin-api/applications 401
    want_status "${node_host}" /admin-api/applications "${refused}" -H "Origin: http://${app_host}"
    # A loopback dev page is not an app origin: auth answers it, the origin guard does not.
    want_status "${node_host}" /admin-api/applications 401 -H 'Origin: http://localhost:5173'

    [ -z "${apps_port}" ] || check_apps_port
}

# The apps port serves files only, login included. A container does not publish it, so it is checked on a native node only.
check_apps_port() {
    docker port "${node}" >/dev/null 2>&1 && return
    apps_url="http://127.0.0.1:${apps_port}"
    for path in /jsonrpc /ws /sse /auth/token /admin-api/contexts; do
        files_only -X POST "${apps_url}${path}"
    done
    files_only "${apps_url}/auth/login?app-id=x"
    status=$(curl -sS -o /dev/null -w '%{http_code}' "${apps_url}/${label}/${release_a}/") || fail "no answer on the apps port"
    [ "${status}" = 200 ] || fail "the apps port did not serve its files: ${status}"
}

# files_only <curl args>: the apps port must refuse the request, saying it serves files only.
files_only() {
    status=$(curl -sS -o "${body}" -w '%{http_code}' "$@") || fail "no answer on the apps port: $*"
    [ "${status}" = "${refused}" ] || fail "the apps port answered ${status}, wanted ${refused}: $*"
    grep -q 'files only' "${body}" || fail "the apps port's refusal does not say why: $* $(cat "${body}")"
}

check_proxy() {
    want_status "${app_host}" /jsonrpc "${refused}" -X POST -H 'Content-Type: application/json' -d '{}'
    want_header 'content-type: text/plain' "/jsonrpc without a token"
    [ -s "${body}" ] || fail "the refusal has no body"
    want_status "${app_host}" /admin-api/contexts "${refused}"
    want_status "${app_host}" /admin-api/contexts/abc "${refused}" -X DELETE
    want_status "${app_host}" /auth/login 302
    want_status "${node_host}" /admin-api/applications "${refused}" -H "Origin: http://${app_host}"
    fetch "${node_host}" /admin-api/applications -H 'Origin: http://localhost:5173'
    [ "${status}" != "${refused}" ] || fail "a loopback dev page was refused as an app origin"
}

case "${mode}" in
    embedded) check_embedded ;;
    proxy) check_proxy ;;
    *) fail "unknown mode ${mode}" ;;
esac

echo "OK (${mode}): ${app_host}"

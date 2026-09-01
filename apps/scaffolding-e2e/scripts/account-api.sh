#!/bin/sh
#
# Shared helpers for account-pair-refusal-statuses.sh. Sourced, not executed.
#
# Uses curl against the admin API rather than meroctl: the merod image ships no
# CLI, so a `target: local` script has none to call.
node_url() {
    _container="$1"
    _hostport=$(docker port "${_container}" 2528/tcp 2>/dev/null | head -1 | sed 's/.*://')
    if [ -z "${_hostport}" ]; then
        echo "could not resolve published RPC port for ${_container}" >&2
        return 1
    fi
    echo "http://127.0.0.1:${_hostport}"
}

# Authenticate and echo a bearer token.
#
# Mirrors merobox's own login payload exactly (`auth_method`, `public_key`,
# `client_name`, `timestamp`, `provider_data`) - the node rejects anything else,
# and a mismatch here would look like a node fault rather than a bad request.
node_token() {
    _url="$1"
    _resp=$(curl -sS --fail-with-body -X POST "${_url}/auth/token" \
        -H 'Content-Type: application/json' \
        -d "{\"auth_method\":\"user_password\",\"public_key\":\"dev\",\
\"client_name\":\"${_url}\",\"timestamp\":$(date +%s),\
\"provider_data\":{\"username\":\"dev\",\"password\":\"dev-password\"}}") || {
        echo "auth failed for ${_url}: ${_resp}" >&2
        return 1
    }
    _token=$(echo "${_resp}" | jq -r '.data.access_token // .access_token // empty')
    if [ -z "${_token}" ]; then
        echo "no access_token in auth response: ${_resp}" >&2
        return 1
    fi
    echo "${_token}"
}

# Call an admin-api path on a node, with an optional JSON body. Echoes the
# response body and fails the call on any 4xx.
api() {
    _container="$1"
    _method="$2"
    _path="$3"

    _url=$(node_url "${_container}") || return 1
    _token=$(node_token "${_url}") || return 1

    # Rebinding this function's own positionals is how a body is passed to curl
    # as one argument without word-splitting an unquoted expansion.
    if [ "$#" -ge 4 ]; then
        set -- -d "$4"
    else
        set --
    fi

    _resp=$(curl -sS --fail-with-body -X "${_method}" "${_url}/admin-api/${_path}" \
        -H 'Content-Type: application/json' \
        -H "Authorization: Bearer ${_token}" "$@") || {
        echo "${_method} ${_path} on ${_container} failed: ${_resp}" >&2
        return 1
    }
    echo "${_resp}"
}

# The same call, echoing the HTTP status and discarding the response.
#
# `api` fails the call on any 4xx, which answers "was it refused" but not "how"
# - and the difference between a 400 and a 500 is the difference between a
# refusal a client can act on and a node that looks broken. The account-scoped
# reads need it for the same reason: `404` for a node with no account at all is
# a different statement from a node that answers with one.
api_status() {
    _container="$1"
    _method="$2"
    _path="$3"

    _url=$(node_url "${_container}") || return 1
    _token=$(node_token "${_url}") || return 1

    if [ "$#" -ge 4 ]; then
        set -- -d "$4"
    else
        set --
    fi

    curl -sS -o /dev/null -w '%{http_code}' -X "${_method}" "${_url}/admin-api/${_path}" \
        -H 'Content-Type: application/json' \
        -H "Authorization: Bearer ${_token}" "$@"
}

# POST a body that must be refused, and assert the status it is refused with.
#
# Every refusal in these scenarios is asserted this way rather than by failing
# the call: a `200` is a security regression and a `500` is the regression the
# status mapping exists to prevent, so both have to fail the run.
expect_status() {
    _want="$1"
    _container="$2"
    _path="$3"
    _body="$4"
    _what="$5"

    _got=$(api_status "${_container}" POST "${_path}" "${_body}") || return 1
    if [ "${_got}" != "${_want}" ]; then
        echo "POST ${_path} on ${_container} answered ${_got} to ${_what}, expected ${_want}" >&2
        exit 1
    fi
    echo "${_what} refused with ${_want}, as it must be"
}

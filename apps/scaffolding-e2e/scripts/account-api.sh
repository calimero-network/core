#!/bin/sh
#
# Shared helpers for the account-identity e2e scripts.
#
# These talk to a node's admin API directly with curl rather than through
# `meroctl`. That is not a shortcut: nothing else in the e2e suite invokes
# meroctl — merobox drives nodes through its own API client — and the merod
# image does not ship the CLI, so a `target: local` script has no meroctl to
# call. The commands under test are thin wrappers over these same endpoints, so
# exercising the endpoints exercises the same code paths.
#
# Sourced, not executed.

# The host URL for a merobox-managed node container.
#
# merobox publishes each node's RPC port on an ephemeral host port, so the port
# cannot be assumed from the node index — ask Docker what it actually bound.
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
# `client_name`, `timestamp`, `provider_data`) — the node rejects anything else,
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

# POST a JSON body to an admin-api path on a node. Echoes the response body.
api_post() {
    _container="$1"
    _path="$2"
    _body="$3"

    _url=$(node_url "${_container}") || return 1
    _token=$(node_token "${_url}") || return 1

    _resp=$(curl -sS --fail-with-body -X POST "${_url}/admin-api/${_path}" \
        -H 'Content-Type: application/json' \
        -H "Authorization: Bearer ${_token}" \
        -d "${_body}") || {
        echo "POST ${_path} on ${_container} failed: ${_resp}" >&2
        return 1
    }
    echo "${_resp}"
}

# GET an admin-api path on a node. Echoes the response body.
api_get() {
    _container="$1"
    _path="$2"

    _url=$(node_url "${_container}") || return 1
    _token=$(node_token "${_url}") || return 1

    _resp=$(curl -sS --fail-with-body "${_url}/admin-api/${_path}" \
        -H "Authorization: Bearer ${_token}") || {
        echo "GET ${_path} on ${_container} failed: ${_resp}" >&2
        return 1
    }
    echo "${_resp}"
}

# PUT a JSON body to an admin-api path on a node. Echoes the response body.
api_put() {
    _container="$1"
    _path="$2"
    _body="$3"

    _url=$(node_url "${_container}") || return 1
    _token=$(node_token "${_url}") || return 1

    _resp=$(curl -sS --fail-with-body -X PUT "${_url}/admin-api/${_path}" \
        -H 'Content-Type: application/json' \
        -H "Authorization: Bearer ${_token}" \
        -d "${_body}") || {
        echo "PUT ${_path} on ${_container} failed: ${_resp}" >&2
        return 1
    }
    echo "${_resp}"
}

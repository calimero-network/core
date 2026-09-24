#!/bin/sh
#
# Shared helpers for the scripts that drive a node over HTTP. Sourced, not executed.
#
# Uses curl against the admin API rather than meroctl: the merod image ships no
# CLI, so a `target: local` script has none to call.

# The merod merobox runs in binary mode. Not executable means a Docker run.
MEROD_BIN="${MEROD_BIN:-../../target/debug/merod}"

# Abandon the run, naming what did not hold.
fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# The node's admin URL. A Docker node publishes its RPC port; a binary-mode
# node records it in the config merobox wrote under ./data.
node_url() {
    _node="$1"
    _hostport=$(docker port "${_node}" 2528/tcp 2>/dev/null | head -1 | sed 's/.*://')
    if [ -z "${_hostport}" ]; then
        # Searched, not spelled out: merobox has moved the config's depth under
        # ./data between releases, and it writes `listen` as a multi-line array.
        _config=$(find "data/${_node}" -name config.toml 2>/dev/null | head -1)
        if [ -n "${_config}" ]; then
            _hostport=$(awk '/^\[server\]/{f=1;next} /^\[/{f=0} f' "${_config}" \
                | grep -o '/tcp/[0-9]*' | head -1 | cut -d/ -f3)
        fi
    fi
    if [ -z "${_hostport}" ]; then
        echo "could not resolve the RPC port for ${_node}" >&2
        return 1
    fi
    echo "http://127.0.0.1:${_hostport}"
}

# Run a node-free `merod` subcommand, in binary or Docker mode.
#
# `account warrant` and `account login-statement` sign from their flags alone --
# no home, no store, no node. They take no node argument here because they read
# none: this used to hunt for a node home with `find` and bind-mount it into the
# container purely to satisfy merod's then-required `--node`, which those
# commands never consulted. A home that is never opened cannot be the wrong one,
# so the search (and its one past breakage, when merobox moved the home) went
# with the flag.
offline_merod() {
    if [ -x "${MEROD_BIN}" ]; then
        "${MEROD_BIN}" "$@"
        return
    fi
    # No bind mount, so no `--user root`: that existed to reach a root-owned
    # `data/` and the image's own `USER user` can run merod perfectly well.
    docker run --rm --entrypoint "" \
        "${MEROD_IMAGE:-merod:local}" \
        merod "$@"
}

# Mint a delegated session for a device key, with no password.
#
# Echoes the access token on stdout and nothing else, so a caller can take it
# with `$(...)`; progress goes to stderr. Shared rather than copied because the
# exchange has four moving parts that must agree (challenge, statement, session
# key, credential), and a second copy drifts silently — it would keep passing
# its own checks while real clients were refused.
#
# ONE `login-statement` invocation: each call signs a fresh statement with its
# own timestamps, so taking the statement from one and the keys from another
# posts a statement naming a session key it never signed over.
mint_session() {
    _url="$1"
    _node_key="$2"
    _secret="$3"
    _credential="$4"

    # Fetched immediately before signing: single-use and short-lived, so a
    # statement minted against a stale one is refused before any signature work.
    _challenge=$(curl -fsS "${_url}/auth/challenge" \
        | sed -n 's/.*"challenge"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)".*/\1/p')
    [ -n "${_challenge}" ] || fail "the node issued no challenge"

    _signed=$(offline_merod account login-statement \
        --challenge "${_challenge}" \
        --node "${_node_key}" \
        --device-secret "${_secret}" \
        --generate-session-key \
        --credential "${_credential}" \
        --audience cli)

    _statement=$(echo "${_signed}" | head -1)
    _session_key=$(echo "${_signed}" | sed -n 's/^Session:[[:space:]]*//p')
    [ -n "${_statement}" ] || fail "merod produced no login statement"
    [ -n "${_session_key}" ] || fail "merod produced no session key"
    echo "statement signed, session key ${_session_key}" >&2

    # `timestamp` is REQUIRED and `BaseTokenRequest` is `deny_unknown_fields`, so
    # a body missing it is rejected before any provider runs -- and the refusal
    # names deserialization, not the login, which reads as though the statement
    # were at fault. `permissions` is deliberately OMITTED rather than set:
    # leaving it unset takes the provider's own `session_permissions` instead of
    # asking for authority a delegated session must not have.
    _body=$(printf '{"auth_method":"account_proof","public_key":"%s","client_name":"%s","timestamp":%s,"provider_data":{"challenge":"%s","login_statement":"%s","account_proof":"%s"}}' \
        "${_session_key}" "${_url}" "$(date +%s)" "${_challenge}" "${_statement}" "${_credential}")

    _res=$(curl -sS -X POST "${_url}/auth/token" \
        -H 'Content-Type: application/json' \
        -d "${_body}")
    _token=$(echo "${_res}" | sed -n 's/.*"access_token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    [ -n "${_token}" ] || fail "no session was minted: ${_res}"
    echo "${_token}"
}

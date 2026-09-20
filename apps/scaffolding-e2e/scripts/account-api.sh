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

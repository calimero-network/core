#!/bin/sh
#
# Turns outbound requests on for one node, as its operator would: appends
# `[context.outbound]` to its config.toml. The node reads it on its next start,
# so the workflow stops and starts the node after this.
#
# The table uses the test build's `[context.outbound.testing]`: `.test` hosts are
# answered in-process with an echo of what the node sent, and ECHO_KEY is bound
# to the installed probe app's package and signer, and to the echo host.
#
#   $1  node name        $2  the node's admin URL      $3  the probe's application id
set -eu

node="$1"
url="$2"
app_id="$3"

signer=$(curl -fsS "${url}/admin-api/applications/${app_id}" \
    | jq -r '[.. | .signer_id? // empty][0] // empty')
[ -n "${signer}" ] || { echo "FAIL: ${app_id} has no signer on ${node}" >&2; exit 1; }

block=$(cat <<TOML

[context.outbound]
enabled = true
hosts = ["echo.outbound.test"]

[context.outbound.testing]
echo = true

[context.outbound.testing.secrets.ECHO_KEY]
value = "s3cr3t-e2e"
packages = ["com.calimero.outbound-probe"]
signers = ["${signer}"]
hosts = ["echo.outbound.test"]
TOML
)

if docker inspect "${node}" >/dev/null 2>&1; then
    # A Docker node: its data dir belongs to the container's user, so append
    # through a container that mounts the node's volumes.
    printf '%s\n' "${block}" | docker run --rm -i --volumes-from "${node}" alpine:latest \
        sh -c "cat >> /app/data/${node}/config.toml && grep -q 'context.outbound.testing' /app/data/${node}/config.toml"
else
    # A binary-mode node: merobox writes its config under ./data.
    config=$(find "data/${node}" -name config.toml 2>/dev/null | head -1)
    [ -n "${config}" ] || { echo "FAIL: no config.toml for ${node}" >&2; exit 1; }
    printf '%s\n' "${block}" >> "${config}"
fi
echo "outbound enabled on ${node} for signer ${signer}"

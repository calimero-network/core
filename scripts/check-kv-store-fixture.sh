#!/usr/bin/env bash
# Fail unless apps/kv-store, the source of mero-mcp's fixture and the release bundle,
# documents every method and parameter and ships a guide with the five required sections.
set -euo pipefail

readonly SECTIONS=("Overview" "Context model" "Getting started" "Procedures" "Rules and limits")

cd "$(git rev-parse --show-toplevel)"
PATH="$(scripts/setup-cargo-mero.sh):$PATH"

mpk="$(mktemp -d)/kv-store.mpk"
cargo mero bundle --dev --no-icon --manifest-path apps/kv-store/Cargo.toml --output "$mpk"
abi="$(tar -xOzf "$mpk" abi.json)"
manifest="$(tar -xOzf "$mpk" manifest.json)"

status=0
fail() {
    echo "ERROR: $1" >&2
    status=1
}

undocumented="$(jq -r '.methods[] | select(.doc == null or any(.params[]; .doc == null)) | .name' <<<"$abi")"
[ -z "$undocumented" ] || fail "kv-store methods missing a doc on the method or a parameter: $(tr '\n' ' ' <<<"$undocumented")"

if ! guide="$(jq -er '.metadata.guide' <<<"$manifest")"; then
    fail "the kv-store bundle carries no metadata.guide"
else
    for section in "${SECTIONS[@]}"; do
        grep -qxE "## ${section}[[:space:]]*" <<<"$guide" || fail "the kv-store guide has no '## ${section}'"
    done
    awk '/^## /{inside = ($0 ~ /^## Procedures[[:space:]]*$/)} inside && /^### /{n++} END{exit !(n > 0)}' <<<"$guide" \
        || fail "the kv-store guide's '## Procedures' has no '###' procedure"
fi

exit "$status"

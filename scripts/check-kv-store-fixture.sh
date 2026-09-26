#!/usr/bin/env bash
# Fail unless apps/kv-store, the source of mero-mcp's fixture and the release bundle,
# documents every method, parameter and return value and ships a guide in the required format.
set -euo pipefail

readonly SECTIONS=("Overview" "Context model" "Getting started" "Procedures" "Rules and limits")
readonly MAX_GUIDE_BYTES=16384

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

undocumented_returns="$(jq -r '.methods[] | select(.returns.kind != "unit" and .returns_doc == null) | .name' <<<"$abi")"
[ -z "$undocumented_returns" ] || fail "kv-store methods returning a value have no returns_doc: $(tr '\n' ' ' <<<"$undocumented_returns")"
jq -e 'any(.methods[]; .returns_doc != null)' <<<"$abi" >/dev/null || fail "no kv-store method has a returns_doc"

if ! guide="$(jq -er '.metadata.guide' <<<"$manifest")"; then
    fail "the kv-store bundle carries no metadata.guide"
else
    bytes="$(jq -r '.metadata.guide | utf8bytelength' <<<"$manifest")"
    [ "$bytes" -le "$MAX_GUIDE_BYTES" ] || fail "the kv-store guide is ${bytes} bytes, over the ${MAX_GUIDE_BYTES} byte limit"
    # Headings inside ``` fences are examples, not sections (the registry's rule).
    unfenced="$(awk '/^ *```/{fenced = !fenced; next} !fenced' <<<"$guide")"
    for section in "${SECTIONS[@]}"; do
        grep -qxE "## ${section}[[:space:]]*" <<<"$unfenced" || fail "the kv-store guide has no '## ${section}'"
    done
    awk '/^## /{inside = ($0 ~ /^## Procedures[[:space:]]*$/)} inside && /^### /{n++} END{exit !(n > 0)}' <<<"$unfenced" \
        || fail "the kv-store guide's '## Procedures' has no '###' procedure"
fi

exit "$status"

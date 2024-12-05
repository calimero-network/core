#!/bin/sh
set -e

cd "$(dirname $0)"

near contract deploy \
    calimero-package-manager.testnet \
    use-file ./res/calimero_registry.wasm \
    without-init-call \
    network-config testnet \
    sign-with-legacy-keychain \
    send

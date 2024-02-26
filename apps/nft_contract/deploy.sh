#!/usr/bin/env bash

cd "$(dirname "$0")"

near dev-deploy --wasmFile res/nft_contract.wasm
#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -ex

cd "$(dirname $0)"

# prepare apps
./build-all-apps.sh

# Prepare contracts
../contracts/near/registry/build.sh
../contracts/near/context-config/build.sh
../contracts/near/context-proxy/build-test-deps.sh
../contracts/icp/context-config/build.sh
../contracts/icp/context-proxy/build_contracts.sh
../contracts/stellar/context-config/build_all_contracts.sh
# Run cargo test
cargo test

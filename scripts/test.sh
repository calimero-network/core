#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -ex

cd "$(dirname $0)"

# prepare apps
./build-all-apps.sh

# Prepare contracts
../contracts/registry/build.sh
../contracts/context-config/build.sh

# Run cargo test
cargo test

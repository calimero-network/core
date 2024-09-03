#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -e

cd "$(dirname $0)"

# prepare apps
./build-all-apps.sh

# Prepare package manager
./../contracts/package-manager/build.sh
./../contracts/context-config/build.sh

# Run cargo test
cargo test

#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -e

# prepare apps
./scripts/build-all-apps.sh

# Prepare package manager
./contracts/package-manager/build.sh

# Run cargo test
cargo test

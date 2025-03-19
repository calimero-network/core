#!/bin/bash

# This script is used to initialise a local Stellar devnet with Calimero contracts.
# Required dependencies: docker
# Usage: ./nuke-devnet.sh [contracts_dir]

set -e

if ! command -v docker &> /dev/null; then
    echo "Error: docker is not installed"
    exit 1
fi

# Check if Docker daemon is running
if ! docker info >/dev/null 2>&1; then
    echo "Error: Docker daemon is not running"
    exit 1
fi

docker stop stellar

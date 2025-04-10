#!/bin/bash
set -e

echo "Stopping zkSync local node..."
zksync-cli local down

echo "Cleaning up zkSync CLI..."
sudo rm -f /usr/local/bin/zksync-cli

echo "Cleaning up contract deployments..."
rm -rf contracts/zksync/deployments/localhost 
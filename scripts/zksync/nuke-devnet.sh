#!/bin/bash

# This script is used to stop a local ZKsync Era devnet deployed with deploy-devnet.sh
# Usage: ./nuke-devnet.sh

pkill -f "anvil-zksync" 2>/dev/null && echo "ZKsync Era processes stopped" || echo "No ZKsync Era processes found"
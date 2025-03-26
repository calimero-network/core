#!/bin/bash
pkill -f anvil 2>/dev/null && echo "Anvil stopped" || echo "No Anvil process found"

#!/bin/bash

# Simple script to test WebSocket authentication via Sec-WebSocket-Protocol.
# Usage: ./test-websocket-auth.sh <YOUR_JWT_TOKEN>

# --- Configuration ---
HOST="localhost"
PORT="80"
ENDPOINT="/ws"
REAL_PROTOCOL="calimero-chat-v1" # The "real" protocol our application might use.
# ---------------------

# Check if token is provided
if [ -z "$1" ]; then
  echo "Error: No JWT token provided."
  echo "Usage: $0 <YOUR_JWT_TOKEN>"
  exit 1
fi

TOKEN="$1"
URL="ws://${HOST}:${PORT}${ENDPOINT}"

echo "Attempting to connect to: ${URL}"
echo "Using protocol smuggling with token..."

# Use websocat to test the connection.
# The token is passed as a custom "protocol" alongside the real one.
if echo "test" | websocat -t --exit-on-eof --protocol "${REAL_PROTOCOL}, ${TOKEN}" "${URL}"; then
  echo "✅ WebSocket connection successful."
else
  echo "❌ WebSocket connection failed."
  exit 1
fi

# Test connection without a token (should fail)
echo -e "\nAttempting to connect without a token (this should fail)..."

if echo "test" | websocat -t --exit-on-eof --protocol "${REAL_PROTOCOL}" "${URL}" 2>/dev/null; then
    echo "❌ Test Failed: Connection succeeded without a token."
    exit 1
else
    echo "✅ Test Passed: Connection correctly failed without a token."
fi 
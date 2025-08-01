#!/bin/bash

# Simple script to test WebSocket authentication.
# Usage: ./test-websocket-auth.sh <YOUR_JWT_TOKEN>

# --- Configuration ---
HOST="localhost"
PORT="80"
ENDPOINT="/ws"
# ---------------------

# Check if token is provided
if [ -z "$1" ]; then
  echo "Error: No JWT token provided."
  echo "Usage: $0 <YOUR_JWT_TOKEN>"
  exit 1
fi

TOKEN="$1"
URL="ws://${HOST}:${PORT}${ENDPOINT}?token=${TOKEN}"

echo "Attempting to connect to: ${URL}"

# Use websocat to test the connection.
# The -t flag allows us to send a test message.
# The --exit-on-eof flag ensures the script exits after the server closes the connection.
if echo "test" | websocat -t --exit-on-eof "${URL}"; then
  echo "✅ WebSocket connection successful."
else
  echo "❌ WebSocket connection failed."
  exit 1
fi

# Test connection without a token (should fail)
echo -e "\nAttempting to connect without a token (this should fail)..."
URL_NO_TOKEN="ws://${HOST}:${PORT}${ENDPOINT}"

if echo "test" | websocat -t --exit-on-eof "${URL_NO_TOKEN}" 2>/dev/null; then
    echo "❌ Test Failed: Connection succeeded without a token."
    exit 1
else
    echo "✅ Test Passed: Connection correctly failed without a token."
fi 
#!/bin/bash
# Simple script to get mock tokens for CI/testing

AUTH_URL="${AUTH_URL:-http://localhost/auth}"
NODE_URL="${NODE_URL:-http://node1.127.0.0.1.nip.io}"
AUTH_TOKEN="${MOCK_AUTH_TOKEN:-Bearer test-auth-token}"
CLIENT_NAME="${CLIENT_NAME:-ci-test-client}"

# Get tokens
RESPONSE=$(curl -s -X POST "$AUTH_URL/mock-token" \
  -H "Content-Type: application/json" \
  -H "Authorization: $AUTH_TOKEN" \
  -d "{
    \"client_name\": \"$CLIENT_NAME\",
    \"permissions\": [\"admin\"],
    \"node_url\": \"$NODE_URL\"
  }")

# Extract and output tokens
ACCESS_TOKEN=$(echo $RESPONSE | jq -r '.data.access_token')
REFRESH_TOKEN=$(echo $RESPONSE | jq -r '.data.refresh_token')

if [ "$ACCESS_TOKEN" != "null" ] && [ -n "$ACCESS_TOKEN" ]; then
    echo "ACCESS_TOKEN=$ACCESS_TOKEN"
    echo "REFRESH_TOKEN=$REFRESH_TOKEN"
    
    # Also set as environment variables
    export ACCESS_TOKEN
    export REFRESH_TOKEN
else
    echo "Failed to get tokens: $RESPONSE" >&2
    exit 1
fi

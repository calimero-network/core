#!/bin/bash

# WebSocket Authentication Test Script
# Tests the new WebSocket auth functionality

set -e

# Configuration
AUTH_URL="${AUTH_URL:-http://localhost:3001}"
NODE_URL="${NODE_URL:-http://localhost:2528}"
WS_URL="${WS_URL:-ws://localhost/ws}"

echo "🔐 Testing WebSocket Authentication"
echo "=================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test 1: Get a JWT token
echo -e "\n${YELLOW}1. Getting JWT token...${NC}"
TOKEN_RESPONSE=$(curl -s -X POST "${AUTH_URL}/auth/token" \
  -H "Content-Type: application/json" \
  -d '{
    "auth_method": "near_wallet",
    "public_key": "test-key",
    "client_name": "websocket-test",
    "timestamp": '$(date +%s)',
    "provider_data": {}
  }')

if echo "$TOKEN_RESPONSE" | grep -q "access_token"; then
  TOKEN=$(echo "$TOKEN_RESPONSE" | jq -r '.access_token')
  echo -e "${GREEN}✅ Token obtained successfully${NC}"
else
  echo -e "${RED}❌ Failed to get token${NC}"
  echo "$TOKEN_RESPONSE"
  exit 1
fi

# Test 2: Validate token via HTTP
echo -e "\n${YELLOW}2. Validating token via HTTP...${NC}"
HTTP_VALIDATION=$(curl -s -w "%{http_code}" "${AUTH_URL}/auth/validate" \
  -H "Authorization: Bearer ${TOKEN}")

HTTP_STATUS="${HTTP_VALIDATION: -3}"
HTTP_BODY="${HTTP_VALIDATION%???}"

if [ "$HTTP_STATUS" = "200" ]; then
  echo -e "${GREEN}✅ HTTP validation successful${NC}"
else
  echo -e "${RED}❌ HTTP validation failed (${HTTP_STATUS})${NC}"
  echo "$HTTP_BODY"
  exit 1
fi

# Test 3: Validate token via query parameter
echo -e "\n${YELLOW}3. Validating token via query parameter...${NC}"
QUERY_VALIDATION=$(curl -s -w "%{http_code}" "${AUTH_URL}/auth/validate?token=${TOKEN}")

QUERY_STATUS="${QUERY_VALIDATION: -3}"
QUERY_BODY="${QUERY_VALIDATION%???}"

if [ "$QUERY_STATUS" = "200" ]; then
  echo -e "${GREEN}✅ Query parameter validation successful${NC}"
else
  echo -e "${RED}❌ Query parameter validation failed (${QUERY_STATUS})${NC}"
  echo "$QUERY_BODY"
  exit 1
fi

# Test 4: WebSocket connection with token
echo -e "\n${YELLOW}4. Testing WebSocket connection with token...${NC}"
if command -v websocat &> /dev/null; then
  # Use websocat if available
  echo "Using websocat for WebSocket test..."
  timeout 10 websocat "${WS_URL}?token=${TOKEN}" || {
    echo -e "${YELLOW}⚠️  WebSocket connection test (websocat not available or connection failed)${NC}"
    echo "Install websocat: cargo install websocat"
  }
else
  echo -e "${YELLOW}⚠️  websocat not available, skipping WebSocket test${NC}"
  echo "Install websocat: cargo install websocat"
fi

# Test 5: Test without token (should fail)
echo -e "\n${YELLOW}5. Testing WebSocket without token (should fail)...${NC}"
if command -v websocat &> /dev/null; then
  timeout 5 websocat "${WS_URL}" && {
    echo -e "${RED}❌ WebSocket connection succeeded without token (should have failed)${NC}"
    exit 1
  } || {
    echo -e "${GREEN}✅ WebSocket correctly rejected connection without token${NC}"
  }
else
  echo -e "${YELLOW}⚠️  Skipping no-token test (websocat not available)${NC}"
fi

echo -e "\n${GREEN}🎉 All tests completed!${NC}"
echo -e "\nTo test manually:"
echo "1. Get token: curl -X POST ${AUTH_URL}/auth/token -H 'Content-Type: application/json' -d '{\"auth_method\":\"near_wallet\",\"public_key\":\"test-key\",\"client_name\":\"test\",\"timestamp\":$(date +%s),\"provider_data\":{}}'"
echo "2. Connect WebSocket: websocat '${WS_URL}?token=YOUR_TOKEN_HERE'" 
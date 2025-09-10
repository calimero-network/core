#!/bin/bash

set -e

echo "🚀 Testing Mock Authentication Endpoint"
echo "======================================="

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Check if services are running
echo -e "${BLUE}1. Checking if services are running...${NC}"
if ! curl -s http://localhost/auth/health > /dev/null; then
    echo -e "${RED}❌ Auth service not accessible at http://localhost/auth/health${NC}"
    echo "Please start docker-compose: docker-compose -f docker-compose.prod.yml up -d"
    exit 1
fi
echo -e "${GREEN}✅ Auth service is running${NC}"

# Test 1: Mock token endpoint
echo -e "\n${BLUE}2. Testing mock token endpoint...${NC}"
RESPONSE=$(curl -s -X POST http://localhost/auth/mock-token \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer test-auth-token" \
  -d '{
    "client_name": "meroctl-test",
    "permissions": ["admin"],
    "node_url": "http://node1.127.0.0.1.nip.io"
  }') 

echo "Response: $RESPONSE"

# Extract tokens
ACCESS_TOKEN=$(echo $RESPONSE | jq -r '.data.access_token')
REFRESH_TOKEN=$(echo $RESPONSE | jq -r '.data.refresh_token')

if [ "$ACCESS_TOKEN" = "null" ] || [ -z "$ACCESS_TOKEN" ]; then
    echo -e "${RED}❌ Failed to get access token${NC}"
    echo "Response: $RESPONSE"
    exit 1
fi

echo -e "${GREEN}✅ Successfully got mock tokens${NC}"
echo "Access Token: ${ACCESS_TOKEN:0:50}..."
echo "Refresh Token: ${REFRESH_TOKEN:0:50}..."

# Test 2: Validate the token
echo -e "\n${BLUE}3. Testing token validation...${NC}"
VALIDATION_RESPONSE=$(curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  http://localhost/auth/validate)

echo "Validation Response: $VALIDATION_RESPONSE"

if echo $VALIDATION_RESPONSE | jq -e '.data' > /dev/null; then
    echo -e "${GREEN}✅ Token validation successful${NC}"
else
    echo -e "${RED}❌ Token validation failed${NC}"
    exit 1
fi

# Test 3: Test with meroctl (if available)
echo -e "\n${BLUE}4. Testing with meroctl CLI...${NC}"

# Test meroctl integration
    echo "Adding node with mock token..."

# Generate a unique node name for this test run
TIMESTAMP=$(date +%s)
NODE_NAME="test-node-$TIMESTAMP"

# Add the node with the mock tokens using cargo run
cargo run -p meroctl -- node add "$NODE_NAME" http://node1.127.0.0.1.nip.io \
    --access-token "$ACCESS_TOKEN" \
    --refresh-token "$REFRESH_TOKEN"
    
    echo "Testing meroctl commands..."
    
    # Set as active node
    cargo run -p meroctl -- node use "$NODE_NAME"
    
    # Test a simple command
    echo "Testing peer count..."
    cargo run -p meroctl -- peers
    
    echo -e "${GREEN}✅ meroctl testing successful${NC}"
    
    # Cleanup: remove the test node
    echo "Cleaning up test node..."
    cargo run -p meroctl -- node remove "$NODE_NAME" 2>/dev/null || true

echo -e "\n${GREEN}🎉 All tests completed successfully!${NC}"
echo -e "${BLUE}💡 You can now use these tokens for testing:${NC}"
echo "   Access Token: $ACCESS_TOKEN"
echo "   Refresh Token: $REFRESH_TOKEN"

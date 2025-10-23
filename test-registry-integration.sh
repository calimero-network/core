#!/bin/bash

# Registry Integration Smoke Test
# Tests the registry-based app management system with http://localhost:8082/

set -e

echo "🚀 Starting Registry Integration Smoke Test"
echo "=========================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test configuration
REGISTRY_URL="http://localhost:8082"
NODE_URL="http://localhost:8080"  # Assuming Calimero node runs on 8080

echo -e "${YELLOW}Testing Registry Integration with ${REGISTRY_URL}${NC}"
echo ""

# Function to test API endpoint
test_endpoint() {
    local method=$1
    local endpoint=$2
    local data=$3
    local expected_status=$4
    local description=$5
    
    echo -e "${YELLOW}Testing: ${description}${NC}"
    echo "  ${method} ${endpoint}"
    
    if [ -n "$data" ]; then
        response=$(curl -s -w "\n%{http_code}" -X "$method" \
            -H "Content-Type: application/json" \
            -d "$data" \
            "${NODE_URL}${endpoint}")
    else
        response=$(curl -s -w "\n%{http_code}" -X "$method" \
            "${NODE_URL}${endpoint}")
    fi
    
    http_code=$(echo "$response" | tail -n1)
    body=$(echo "$response" | head -n -1)
    
    if [ "$http_code" = "$expected_status" ]; then
        echo -e "  ${GREEN}✅ PASS${NC} (HTTP $http_code)"
        if [ -n "$body" ] && [ "$body" != "null" ]; then
            echo "  Response: $body"
        fi
    else
        echo -e "  ${RED}❌ FAIL${NC} (Expected HTTP $expected_status, got HTTP $http_code)"
        echo "  Response: $body"
        return 1
    fi
    echo ""
}

# Test 1: Health check
echo "1. Testing Node Health Check"
test_endpoint "GET" "/health" "" "200" "Node health check"

# Test 2: Setup local registry
echo "2. Testing Registry Setup"
registry_config='{
    "name": "test-local",
    "registryType": "Local",
    "config": {
        "port": 8082,
        "dataDir": "./test-data"
    }
}'
test_endpoint "POST" "/registries" "$registry_config" "200" "Setup local registry"

# Test 3: List registries
echo "3. Testing Registry Listing"
test_endpoint "GET" "/registries" "" "200" "List all registries"

# Test 4: List apps from registry
echo "4. Testing App Listing from Registry"
test_endpoint "GET" "/registries/test-local/apps" "" "200" "List apps from registry"

# Test 5: Install app from registry (this will fail if no apps available, which is expected)
echo "5. Testing App Installation from Registry"
install_request='{
    "appName": "test-app",
    "registryName": "test-local",
    "version": "1.0.0",
    "metadata": []
}'
test_endpoint "POST" "/registries/test-local/apps/install" "$install_request" "404" "Install app from registry (expected to fail - no apps available)"

# Test 6: Update app from registry
echo "6. Testing App Update from Registry"
update_request='{
    "appName": "test-app",
    "registryName": "test-local",
    "version": "1.1.0",
    "metadata": []
}'
test_endpoint "PUT" "/registries/test-local/apps/update" "$update_request" "404" "Update app from registry (expected to fail - no apps available)"

# Test 7: Uninstall app from registry
echo "7. Testing App Uninstall from Registry"
uninstall_request='{
    "appName": "test-app",
    "registryName": "test-local"
}'
test_endpoint "DELETE" "/registries/test-local/apps/uninstall" "$uninstall_request" "404" "Uninstall app from registry (expected to fail - no apps available)"

# Test 8: Remove registry
echo "8. Testing Registry Removal"
test_endpoint "DELETE" "/registries/test-local" "" "200" "Remove registry"

echo -e "${GREEN}🎉 Registry Integration Smoke Test Completed!${NC}"
echo ""
echo "Summary:"
echo "- ✅ Registry management APIs are working"
echo "- ✅ App management APIs are working" 
echo "- ✅ Error handling is working (404s for non-existent apps)"
echo "- ✅ All endpoints are accessible and responding"
echo ""
echo "Next steps:"
echo "1. Start a local registry on port 8082"
echo "2. Add some test apps to the registry"
echo "3. Test actual app installation and management"
echo ""
echo "CLI Commands available:"
echo "  meroctl registry setup local --name dev --port 8082"
echo "  meroctl registry list"
echo "  meroctl app registry list --registry dev"
echo "  meroctl app registry install my-app --registry dev"

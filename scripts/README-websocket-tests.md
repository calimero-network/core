# WebSocket Authentication Test Scripts

Simple test scripts to verify WebSocket authentication functionality.

## Quick Test

### Bash Script
```bash
# Run the bash test script
./scripts/test-websocket-auth.sh

# Or with custom URLs
AUTH_URL=http://your-auth:3001 WS_URL=ws://your-domain/ws ./scripts/test-websocket-auth.sh
```

### Node.js Script
```bash
# Install ws package if needed
npm install ws

# Run the Node.js test script
node scripts/test-websocket-auth.js

# Or with custom URLs
AUTH_URL=http://your-auth:3001 WS_URL=ws://your-domain/ws node scripts/test-websocket-auth.js
```

## What the Tests Do

1. **Get JWT token** from auth service
2. **Validate token via HTTP** (Authorization header)
3. **Validate token via query parameter** (new WebSocket feature)
4. **Test WebSocket connection** with token
5. **Test WebSocket connection** without token (should fail)

## Manual Testing

### Get a Token
```bash
curl -X POST http://localhost:3001/auth/token \
  -H "Content-Type: application/json" \
  -d '{
    "auth_method": "near_wallet",
    "public_key": "test-key",
    "client_name": "test",
    "timestamp": 1234567890,
    "provider_data": {}
  }'
```

### Test WebSocket Connection
```bash
# Install websocat
cargo install websocat

# Connect with token
websocat "ws://localhost/ws?token=YOUR_TOKEN_HERE"
```

## Requirements

- `curl` (for HTTP requests)
- `jq` (for JSON parsing in bash script)
- `websocat` (optional, for WebSocket testing)
- `node` and `ws` package (for Node.js script) 
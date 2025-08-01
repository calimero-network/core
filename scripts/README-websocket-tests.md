# WebSocket Authentication Test Scripts

Simple test scripts to verify WebSocket authentication functionality.

## Quick Test

### Node.js Script (Recommended)
```bash
# Install ws package if needed
npm install ws

# Run with your JWT token
node scripts/test-websocket-auth.js "your-jwt-token-here"

# Or with custom WebSocket URL
WS_URL=ws://your-domain/ws node scripts/test-websocket-auth.js "your-jwt-token-here"
```

### Bash Script (Alternative)
```bash
# Run the bash test script (includes token generation)
./scripts/test-websocket-auth.sh

# Or with custom URLs
AUTH_URL=http://your-auth:3001 WS_URL=ws://your-domain/ws ./scripts/test-websocket-auth.sh
```

## What the Tests Do

### Node.js Script
1. **Test WebSocket connection** with provided token
2. **Test WebSocket connection** without token (should fail)

### Bash Script
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

- `node` and `ws` package (for Node.js script)
- `curl`, `jq`, `websocat` (for bash script) 
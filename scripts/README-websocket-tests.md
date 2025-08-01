# WebSocket Authentication Test

A Node.js package to test WebSocket authentication functionality.

## Quick Test

```bash
# Navigate to the test package
cd scripts/websocket-test

# Install dependencies
npm install

# Run with your JWT token
npm test "your-jwt-token-here"

# Or with custom WebSocket URL
WS_URL=ws://your-domain/ws npm test "your-jwt-token-here"
```

## What the Tests Do

1. **Test WebSocket connection** with provided token
2. **Test WebSocket connection** without token (should fail)

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

- `node` and `npm` (for the test package)
- `curl` (for manual token generation)
- `websocat` (for manual WebSocket testing) 
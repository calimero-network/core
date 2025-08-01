# WebSocket Authentication Test

A simple Node.js package to test WebSocket authentication functionality.

## Quick Start

```bash
# Navigate to the test directory
cd scripts/websocket-test

# Install dependencies
npm install

# Run the test with your JWT token
npm test "your-jwt-token-here"

# Or run directly
node test-websocket-auth.js "your-jwt-token-here"
```

## Usage

```bash
# Test with default WebSocket URL (ws://localhost/ws)
npm test "your-jwt-token-here"

# Test with custom WebSocket URL
WS_URL=ws://your-domain/ws npm test "your-jwt-token-here"

# Test with custom WebSocket URL (direct node command)
WS_URL=ws://your-domain/ws node test-websocket-auth.js "your-jwt-token-here"
```

## What It Tests

1. **WebSocket connection with token** - Should succeed
2. **WebSocket connection without token** - Should fail

## Example Output

```
🔐 Testing WebSocket Authentication
==================================

1. Testing WebSocket connection with token...
Connecting to: ws://localhost/ws?token=your-token-here
✅ WebSocket connection established
✅ WebSocket connection with token successful

2. Testing WebSocket without token (should fail)...
Connecting to: ws://localhost/ws
✅ WebSocket correctly rejected connection without token

🎉 All tests completed successfully!
```

## Environment Variables

- `WS_URL` - WebSocket URL to test (default: `ws://localhost/ws`)

## Dependencies

- `ws` - WebSocket client for Node.js 
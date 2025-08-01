# WebSocket Authentication Test

A simple Node.js package to test WebSocket authentication functionality.

## Quick Start

```bash
# Navigate to the test directory
cd scripts/websocket-test

# Install dependencies
npm install

# Configure your token (see Configuration section below)
# Then run the test
npm test
```

## Configuration

You can configure the token and WebSocket URL in several ways:

### 1. Package.json Config (Recommended)
Edit `package.json` and update the config section:
```json
{
  "config": {
    "token": "your-actual-jwt-token-here",
    "wsUrl": "ws://your-domain/ws"
  }
}
```

### 2. Environment Variables
```bash
# Set token and URL via environment variables
TOKEN=your-jwt-token-here WS_URL=ws://your-domain/ws npm test

# Or export them
export TOKEN=your-jwt-token-here
export WS_URL=ws://your-domain/ws
npm test
```

### 3. Command Line Arguments
```bash
# Pass token as command line argument
npm test "your-jwt-token-here"

# With custom WebSocket URL
WS_URL=ws://your-domain/ws npm test "your-jwt-token-here"
```

## Usage Examples

```bash
# Using package.json config (easiest)
npm test

# Using environment variables
TOKEN=your-token WS_URL=ws://localhost:8080/ws npm test

# Using command line
npm test "your-jwt-token-here"

# Direct node command
node test-websocket-auth.js "your-jwt-token-here"
```

## What It Tests

1. **WebSocket connection with token** - Should succeed
2. **WebSocket connection without token** - Should fail

## Example Output

```
🔐 Testing WebSocket Authentication
==================================
Using WebSocket URL: ws://localhost/ws

1. Testing WebSocket connection with token...
Connecting to: ws://localhost/ws?token=your-token-here
✅ WebSocket connection established
✅ WebSocket connection with token successful

2. Testing WebSocket without token (should fail)...
Connecting to: ws://localhost/ws
✅ WebSocket correctly rejected connection without token

🎉 All tests completed successfully!
```

## Configuration Priority

The script uses the following priority for configuration:
1. **Command line arguments** (highest priority)
2. **Environment variables** (`TOKEN`, `WS_URL`)
3. **Package.json config** (lowest priority)

## Environment Variables

- `TOKEN` - JWT token for authentication
- `WS_URL` - WebSocket URL to test (default: `ws://localhost/ws`)

## Dependencies

- `ws` - WebSocket client for Node.js 
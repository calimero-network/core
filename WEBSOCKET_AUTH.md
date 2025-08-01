# WebSocket Authentication Support

This PR adds WebSocket authentication support to the Calimero network, allowing authenticated WebSocket connections through the existing Traefik reverse proxy infrastructure.

## Overview

WebSocket connections now support the same authentication flow as HTTP requests, using the existing auth service and Traefik forward authentication middleware.

## Changes Made

### 1. Enhanced Auth Service (`crates/auth/src/api/handlers/auth.rs`)

The `/auth/validate` endpoint now supports both:
- **Authorization header**: `Authorization: Bearer <token>` (existing)
- **Query parameter**: `?token=<token>` (new, for WebSocket compatibility)

This allows WebSocket clients to authenticate using query parameters since the WebSocket API doesn't support custom headers.

### 2. Traefik Configuration (`docker-compose.prod.yml`)

Added WebSocket route configuration:
```yaml
# WebSocket route
- "traefik.http.routers.node-ws.rule=PathPrefix(`/ws`)"
- "traefik.http.routers.node-ws.entrypoints=web"
- "traefik.http.routers.node-ws.service=node-core"
- "traefik.http.routers.node-ws.middlewares=cors,auth-node"
```

The WebSocket route uses the same authentication middleware (`auth-node`) as the JSON-RPC and admin routes.

## How It Works

1. **Client** connects to `ws://your-domain/ws?token=your-jwt-token`
2. **Traefik** intercepts the WebSocket upgrade request
3. **Traefik** calls **Auth Service** (`/auth/validate?token=your-jwt-token`) 
4. **Auth Service** validates the token and returns user info in headers
5. **Traefik** forwards the authenticated WebSocket connection to **Node Server**
6. **Node Server** receives the WebSocket connection with user info in headers

## Client Usage

### Method 1: Query Parameter (Recommended for WebSockets)
```javascript
const token = "your-jwt-token";
const ws = new WebSocket(`ws://your-domain/ws?token=${encodeURIComponent(token)}`);

ws.onopen = () => {
    console.log('WebSocket connected with authentication');
};

ws.onmessage = (event) => {
    console.log('Received:', event.data);
};
```

### Method 2: HTTP First, Then WebSocket
```javascript
// First authenticate via HTTP
const response = await fetch('/auth/validate', {
    headers: { 'Authorization': `Bearer ${token}` }
});

if (response.ok) {
    // Token is valid, connect to WebSocket
    const ws = new WebSocket('ws://your-domain/ws');
}
```

## Benefits

- ✅ **Same authentication flow** as HTTP requests
- ✅ **Centralized auth** through the auth service  
- ✅ **Consistent permissions** across HTTP and WebSocket
- ✅ **No changes needed** to the node server WebSocket implementation
- ✅ **Works with existing Traefik setup**
- ✅ **Supports both header and query parameter authentication**

## Testing

1. **Get a valid JWT token** from the auth service
2. **Connect to WebSocket** using the token as a query parameter
3. **Verify authentication** by checking that the connection is established
4. **Test without token** to ensure unauthorized connections are rejected

## Security Considerations

- Tokens in query parameters are visible in server logs and browser history
- Consider using HTTPS/WSS in production to encrypt query parameters
- Tokens should have appropriate expiration times
- The auth service validates tokens and checks key validity/revocation status

## Backward Compatibility

This change is fully backward compatible:
- Existing HTTP authentication continues to work unchanged
- WebSocket connections without tokens will be rejected (as expected)
- No changes required to existing client code that uses HTTP endpoints 
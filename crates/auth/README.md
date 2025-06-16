# Calimero Authentication Service

This is a forward authentication service for Calimero Network. It provides
authentication for web applications and APIs using various authentication
providers.

## Features

- Support for multiple authentication providers:
  - JWT token authentication
  - NEAR wallet authentication
- Client key and root key management
- Permission-based authorization
- Integration with reverse proxies such as Traefik

## Authentication Modes

The service supports two authentication modes:

- `none`: Development mode with no authentication required (all requests pass
  through)
- `forward`: Production mode with full authentication required

## Building

```bash
cargo build --bin calimero-auth
```

## Running

```bash
# Run in development mode (no authentication)
cargo run --bin calimero-auth -- --auth-mode none

# Run in production mode (with authentication)
cargo run --bin calimero-auth -- --auth-mode forward
```

## Docker Setup

The service can be run in Docker using the provided `docker-compose.auth.yml`
file:

```bash
# Build and start the authentication service, Traefik, and Calimero node
docker-compose -f docker-compose.auth.yml up -d

# View logs
docker-compose -f docker-compose.auth.yml logs -f

# Stop and remove containers
docker-compose -f docker-compose.auth.yml down
```

## Testing the Authentication Service

### 1. Start the service

```bash
docker-compose -f docker-compose.auth.yml up -d
```

### 2. Check service status

```bash
# Check service status
curl http://localhost/identity

# Expected response:
# {
#   "node_id": "http://node:3000",
#   "version": "0.1.0",
#   "authentication_mode": "forward"
# }
```

### 3. Test NEAR wallet authentication

To test NEAR wallet authentication, you need to:

1. Create a signature with your NEAR wallet
2. Send a request with the signature and public key

#### Using the NEAR CLI (if available):

```bash
# Generate a signature (you need NEAR CLI installed)
NEAR_ACCOUNT="your-account.near"
MESSAGE="Authentication request for Calimero at $(date +%s)"
NEAR_SIGNATURE=$(near sign "$MESSAGE" $NEAR_ACCOUNT)
NEAR_PUBLIC_KEY=$(near keys $NEAR_ACCOUNT | grep public_key | awk '{print $2}')

# Request a token
curl -X POST http://localhost/auth/token \
  -H "Content-Type: application/json" \
  -H "x-near-account-id: $NEAR_ACCOUNT" \
  -H "x-near-public-key: $NEAR_PUBLIC_KEY" \
  -H "x-near-signature: $NEAR_SIGNATURE" \
  -H "x-near-message: $MESSAGE"

# This should return an access token and refresh token
```

### 4. Test JWT Authentication

After obtaining a token from the previous step:

```bash
# Get an access token from the previous step
ACCESS_TOKEN="your-access-token"

# Test accessing a protected resource
curl -H "Authorization: Bearer $ACCESS_TOKEN" http://localhost/

# If authentication succeeds, you should see the node response
```

### 5. Check the Traefik Dashboard

The Traefik dashboard is available at http://localhost:8080, where you can see
the routing and middleware configuration.

## Configuration

The service can be configured using environment variables or a configuration
file:

```bash
# Using environment variables
AUTH_LISTEN_ADDR=0.0.0.0:3001 \
AUTH_NODE_URL=http://localhost:3000 \
AUTH_STORAGE__TYPE=rocksdb \
AUTH_STORAGE__PATH=./data/auth_db \
cargo run --bin calimero-auth

# Using a configuration file
cargo run --bin calimero-auth -- --config config.toml
```

Example configuration file (`config.toml`):

```toml
listen_addr = "0.0.0.0:3001"
node_url = "http://localhost:3000"

[jwt]
secret = "your-secret-key"
access_token_expiry = 3600
refresh_token_expiry = 2592000
issuer = "auth.calimero.network"

[storage]
type = "rocksdb"
path = "./data/auth_db"

[providers]
jwt = true
near_wallet = true

[providers.near_wallet_config]
rpc_url = "https://rpc.mainnet.near.org"
network_id = "mainnet"

[cors]
allow_all_origins = true
```

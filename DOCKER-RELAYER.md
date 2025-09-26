# Calimero Relayer Docker Guide

This guide covers deploying the Calimero relayer using Docker and Docker Compose.

## Quick Start

### 1. Basic Docker Run

```bash
# Build the image
docker build -f Dockerfile.relayer -t calimero-relayer .

# Run with Near testnet (requires secret key)
docker run -d \
  --name calimero-relayer \
  -p 63529:63529 \
  -e NEAR_DEFAULT_SECRET_KEY="your-secret-key-here" \
  calimero-relayer
```

### 2. Docker Compose (Recommended)

```bash
# Production deployment
export NEAR_DEFAULT_SECRET_KEY="your-secret-key-here"
docker-compose -f docker-compose.relayer.yml up -d

# Development with local blockchain networks
./scripts/relayer/start-dev.sh
```

## Configuration

### Environment Variables

The relayer supports the following environment variables:

#### General Settings
- `RELAYER_LISTEN`: Listen address (default: `0.0.0.0:63529`)
- `RUST_LOG`: Logging level (default: `info`)

#### Protocol Control
- `ENABLE_NEAR`: Enable Near protocol (default: `true`)
- `ENABLE_STARKNET`: Enable Starknet protocol (default: `false`)
- `ENABLE_ICP`: Enable ICP protocol (default: `false`)  
- `ENABLE_ETHEREUM`: Enable Ethereum protocol (default: `false`)

#### Near Protocol
- `NEAR_NETWORK`: Network name (default: `testnet`)
- `NEAR_RPC_URL`: RPC endpoint (default: `https://rpc.testnet.near.org`)
- `NEAR_CONTRACT_ID`: Contract address (default: `calimero-context-config.testnet`)
- `NEAR_ACCOUNT_ID`: Account ID (custom credentials)
- `NEAR_PUBLIC_KEY`: Public key (custom credentials)
- `NEAR_SECRET_KEY`: Secret key (custom credentials)
- `NEAR_DEFAULT_SECRET_KEY`: Default secret key for testnet

#### Starknet Protocol
- `STARKNET_NETWORK`: Network name (default: `sepolia`)
- `STARKNET_RPC_URL`: RPC endpoint
- `STARKNET_CONTRACT_ID`: Contract address
- `STARKNET_ACCOUNT_ID`: Account ID
- `STARKNET_PUBLIC_KEY`: Public key
- `STARKNET_SECRET_KEY`: Secret key
- `STARKNET_DEFAULT_SECRET_KEY`: Default secret key

#### ICP Protocol
- `ICP_NETWORK`: Network name (default: `local`)
- `ICP_RPC_URL`: RPC endpoint (default: `http://host.docker.internal:4943`)
- `ICP_CONTRACT_ID`: Contract ID
- `ICP_ACCOUNT_ID`: Principal ID
- `ICP_PUBLIC_KEY`: Public key
- `ICP_SECRET_KEY`: Secret key
- `ICP_DEFAULT_SECRET_KEY`: Default secret key

#### Ethereum Protocol
- `ETHEREUM_NETWORK`: Network name (default: `sepolia`)
- `ETHEREUM_RPC_URL`: RPC endpoint
- `ETHEREUM_CONTRACT_ID`: Contract address
- `ETHEREUM_ACCOUNT_ID`: Account address
- `ETHEREUM_SECRET_KEY`: Private key
- `ETHEREUM_DEFAULT_SECRET_KEY`: Default private key

## Deployment Scenarios

### 1. Production Deployment

```yaml
# docker-compose.prod.yml
version: '3.8'
services:
  relayer:
    image: calimero-relayer:latest
    ports:
      - "63529:63529"
    environment:
      - ENABLE_NEAR=true
      - NEAR_SECRET_KEY=${NEAR_PROD_SECRET_KEY}
      - ENABLE_ETHEREUM=true  
      - ETHEREUM_SECRET_KEY=${ETHEREUM_PROD_SECRET_KEY}
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:63529/health"]
      interval: 30s
      timeout: 5s
      retries: 3
```

```bash
# Deploy to production
export NEAR_PROD_SECRET_KEY="ed25519:..."
export ETHEREUM_PROD_SECRET_KEY="0x..."
docker-compose -f docker-compose.prod.yml up -d
```

### 2. Development Environment

```bash
# Start development environment with local networks
./scripts/relayer/start-dev.sh

# This will start:
# - ICP dfx local network
# - Ethereum Anvil devnet  
# - Relayer with development credentials
```

### 3. Testing Environment

```bash
# Start with testnet credentials
export NEAR_DEFAULT_SECRET_KEY="ed25519:3D4YudUQRE39Lc4JHghuB5WM8kbgDDa34mnrEP5DdTApVH81af7e2dWgNPEaiQfdJnZq1CNPp5im4Rg5b2rKtXFv"
export STARKNET_DEFAULT_SECRET_KEY="0x0178eb2a625c0a8d85b0a5fd69fc879f9884f5205ad9d1ba41db0d7d1a77950a"

docker-compose -f docker-compose.relayer.yml up -d
```

## Health Monitoring

The relayer includes a health check endpoint:

```bash
# Check health status
curl http://localhost:63529/health

# Expected response:
{
  "status": "healthy",
  "service": "calimero-relayer", 
  "timestamp": "2025-09-15T11:26:22.601119003+00:00"
}
```

### Docker Health Check

The Docker image includes automatic health monitoring:

```bash
# Check container health
docker inspect relayer --format='{{.State.Health.Status}}'

# View health check logs
docker inspect relayer --format='{{range .State.Health.Log}}{{.Output}}{{end}}'
```

## Scaling & Load Balancing

### Multiple Instances

```yaml
version: '3.8'
services:
  relayer-1:
    image: calimero-relayer:latest
    environment:
      - NEAR_SECRET_KEY=${SHARED_NEAR_KEY}
    ports:
      - "63529:63529"
      
  relayer-2:
    image: calimero-relayer:latest
    environment:
      - NEAR_SECRET_KEY=${SHARED_NEAR_KEY}  # Same key for consistency
    ports:
      - "63530:63529"
      
  nginx:
    image: nginx:alpine
    ports:
      - "80:80"
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf
```

### Kubernetes Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: calimero-relayer
spec:
  replicas: 3
  selector:
    matchLabels:
      app: calimero-relayer
  template:
    metadata:
      labels:
        app: calimero-relayer
    spec:
      containers:
      - name: relayer
        image: calimero-relayer:latest
        ports:
        - containerPort: 63529
        env:
        - name: NEAR_SECRET_KEY
          valueFrom:
            secretKeyRef:
              name: relayer-secrets
              key: near-secret-key
        livenessProbe:
          httpGet:
            path: /health
            port: 63529
          initialDelaySeconds: 10
          periodSeconds: 30
        readinessProbe:
          httpGet:
            path: /health
            port: 63529
          initialDelaySeconds: 5
          periodSeconds: 10
```

## Security Best Practices

### 1. Secret Management

```bash
# Never hardcode secrets in Docker files
# Use environment variables or Docker secrets

# Docker secrets (production)
echo "ed25519:secret-key" | docker secret create near_secret_key -
docker service create \
  --secret near_secret_key \
  --env NEAR_SECRET_KEY_FILE=/run/secrets/near_secret_key \
  calimero-relayer

# Kubernetes secrets
kubectl create secret generic relayer-secrets \
  --from-literal=near-secret-key="ed25519:..." \
  --from-literal=ethereum-secret-key="0x..."
```

### 2. Network Security

```yaml
# Use internal networks for service communication
networks:
  internal:
    driver: bridge
    internal: true
  web:
    driver: bridge

services:
  relayer:
    networks:
      - internal
      - web
    # Only expose necessary ports
```

### 3. User Permissions

The Docker image runs as a non-root user (`user:10001`) for security.

## Troubleshooting

### Common Issues

1. **Port already in use**
   ```bash
   # Check what's using the port
   lsof -i :63529
   
   # Use different port mapping
   docker run -p 8080:63529 calimero-relayer
   ```

2. **Missing secret keys**
   ```bash
   # Check logs for specific error
   docker logs calimero-relayer
   
   # Ensure environment variables are set
   docker exec relayer-container env | grep SECRET_KEY
   ```

3. **Health check failing**
   ```bash
   # Test health endpoint manually
   curl http://localhost:63529/health
   
   # Check if service is listening
   docker exec relayer-container netstat -tlnp
   ```

4. **Network connectivity issues**
   ```bash
   # Test RPC connectivity from container
   docker exec relayer-container curl -s https://rpc.testnet.near.org
   ```

### Debug Mode

```bash
# Enable debug logging
docker run -e RUST_LOG=debug calimero-relayer

# Or in docker-compose
environment:
  - RUST_LOG=debug,calimero_relayer=trace
```

## API Usage

Once running, the relayer exposes:

- **POST /** - Main relay endpoint for blockchain requests
- **GET /health** - Health check endpoint

Example API request:
```bash
curl -X POST http://localhost:63529/ \
  -H "Content-Type: application/json" \
  -d '{
    "protocol": "near",
    "network_id": "testnet", 
    "contract_id": "calimero-context-config.testnet",
    "operation": "Read",
    "payload": "{\"method_name\":\"get_contexts\"}"
  }'
```

## Scripts

- `./scripts/relayer/start-dev.sh` - Start development environment
- `./scripts/relayer/start-prod.sh` - Start production environment

Both scripts handle dependency setup (dfx, anvil) and service orchestration.

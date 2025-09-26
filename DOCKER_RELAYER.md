# Docker Relayer Documentation

This document describes the Docker setup and deployment for the Calimero standalone relayer service.

## Overview

The Calimero relayer is containerized using a multi-stage Docker build that creates a production-ready image with security best practices and health monitoring capabilities.

## Docker Image

### Build Arguments

- `RUST_VERSION`: Rust version to use (default: 1.88.0)
- `RELAYER_PORT`: Port for the relayer service (default: 63529)

### Security Features

- **Non-root execution**: Container runs as user `user` (UID: 10001)
- **Environment-only secrets**: No hardcoded credentials in the image
- **Minimal attack surface**: Based on Debian slim with only necessary packages
- **Health monitoring**: Built-in health checks for orchestration

### Health Check

The relayer exposes a health check endpoint at `GET /health` that returns:

```json
{
  "status": "healthy",
  "service": "calimero-relayer", 
  "timestamp": "2025-01-27T10:30:45.123Z"
}
```

**Important**: The timestamp is returned in RFC-3339 format (ISO 8601 with 'Z' suffix for UTC).

The health check runs every 30 seconds with a 5-second timeout and allows 3 retries before marking the container as unhealthy.

## Docker Compose

### Production Deployment

Use `docker-compose.relayer.yml` for production deployments:

```bash
# Set required environment variables
export NEAR_DEFAULT_SECRET_KEY="your-near-secret-key"
export STARKNET_SECRET_KEY="your-starknet-secret-key"  # if using Starknet
export ICP_SECRET_KEY="your-icp-secret-key"           # if using ICP
export ETHEREUM_SECRET_KEY="your-eth-secret-key"      # if using Ethereum

# Start the relayer
docker-compose -f docker-compose.relayer.yml up -d
```

### Development Environment

Use `docker-compose.relayer.dev.yml` for local development with blockchain networks:

```bash
# Set development environment variables
export ETHEREUM_ACCOUNT_ID="your-dev-account-id"
export ETHEREUM_SECRET_KEY="your-dev-secret-key"

# Start development environment
docker-compose -f docker-compose.relayer.yml -f docker-compose.relayer.dev.yml up -d
```

This will start:
- Relayer service
- Ethereum Anvil devnet (if Ethereum is enabled)
- ICP dfx (requires local dfx installation)

## Environment Variables

### Required Variables

All secret keys must be provided via environment variables:

- `NEAR_DEFAULT_SECRET_KEY`: Near protocol secret key
- `STARKNET_SECRET_KEY`: Starknet secret key (if Starknet enabled)
- `ICP_SECRET_KEY`: ICP secret key (if ICP enabled)  
- `ETHEREUM_SECRET_KEY`: Ethereum secret key (if Ethereum enabled)

### Optional Variables

- `RELAYER_PORT`: External port mapping (default: 63529)
- `RELAYER_LISTEN`: Internal listen address (default: 0.0.0.0:63529)
- `RUST_LOG`: Logging level (default: info)
- `DEFAULT_RELAYER_URL`: Relayer URL for client configuration

### Protocol Configuration

Each protocol can be enabled/disabled and configured:

```bash
# Enable protocols
export ENABLE_NEAR=true
export ENABLE_STARKNET=true
export ENABLE_ICP=true
export ENABLE_ETHEREUM=true

# Configure networks and RPC URLs
export NEAR_NETWORK="testnet"
export NEAR_RPC_URL="https://rpc.testnet.near.org"
export STARKNET_NETWORK="sepolia"
export STARKNET_RPC_URL="https://free-rpc.nethermind.io/sepolia-juno/"
export ICP_NETWORK="local"
export ICP_RPC_URL="http://host.docker.internal:4943"
export ETHEREUM_NETWORK="sepolia"
export ETHEREUM_RPC_URL="https://sepolia.drpc.org"

# Set contract IDs (no defaults for security)
export NEAR_CONTRACT_ID="your-near-contract-id"
export STARKNET_CONTRACT_ID="your-starknet-contract-id"
export ICP_CONTRACT_ID="your-icp-contract-id"
export ETHEREUM_CONTRACT_ID="your-eth-contract-id"
```

## Deployment Scripts

### Production Script

```bash
./scripts/relayer/start-prod.sh
```

This script:
1. Checks for dfx installation
2. Starts dfx with ICP support
3. Deploys ICP contracts
4. Starts the relayer container

### Development Script

```bash
./scripts/relayer/start-dev.sh
```

This script:
1. Starts dfx and Ethereum Anvil
2. Deploys both ICP and Ethereum contracts
3. Starts the relayer with development settings

## Monitoring

### Health Check

Monitor container health:

```bash
# Check health status
docker inspect relayer-container --format='{{.State.Health.Status}}'

# View health check logs
docker inspect relayer-container --format='{{range .State.Health.Log}}{{.Output}}{{end}}'

# Test health endpoint directly
curl http://localhost:63529/health
```

### Logs

View relayer logs:

```bash
# Follow logs
docker-compose -f docker-compose.relayer.yml logs -f relayer

# View recent logs
docker-compose -f docker-compose.relayer.yml logs --tail=100 relayer
```

## Security Considerations

1. **Never commit secret keys**: All credentials must be provided via environment variables
2. **Use secrets management**: In production, use Docker secrets or external secret management
3. **Network isolation**: Use Docker networks to isolate the relayer from other services
4. **Regular updates**: Keep the base image and dependencies updated
5. **Health monitoring**: Monitor health check status and logs for anomalies

## Troubleshooting

### Common Issues

1. **Container won't start**: Check that all required environment variables are set
2. **Health check failing**: Verify the relayer is listening on the correct port
3. **Protocol errors**: Ensure RPC URLs are accessible and credentials are valid
4. **Permission errors**: The container runs as non-root user (UID 10001)

### Debug Mode

Enable debug logging:

```bash
export RUST_LOG=debug
docker-compose -f docker-compose.relayer.yml up -d
```

This will provide detailed logs for troubleshooting protocol connections and request handling.

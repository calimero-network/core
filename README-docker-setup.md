# Docker Compose Setup Guide

This repository uses a modular Docker Compose architecture with 4 separate files for maximum flexibility and separation of concerns.

## File Structure

- `docker-compose.auth.yml` - Auth service and Traefik proxy (core infrastructure)
- `docker-compose.nodes.yml` - Node services (1-3 nodes with subdomain routing)
- `docker-compose.config.yml` - Configuration services (setup, build, deployment)
- `docker-compose.prod.yml` - Legacy production setup for 1 node (all-in-one)

## Usage Examples

### 1. Auth Service Only (Core Infrastructure)
```bash
docker-compose -f docker-compose.auth.yml up -d
```
- Runs auth service and Traefik proxy
- Accessible at `http://localhost/auth/` and `http://localhost/admin/`
- Proxy dashboard: `http://proxy.127.0.0.1.nip.io`

### 2. Nodes in Standalone Mode (No Auth Protection)
```bash
docker-compose -f docker-compose.nodes.yml up
```
- Runs 3 nodes without auth service
- All endpoints publicly accessible
- Access points:
  - Node1: `http://node1.127.0.0.1.nip.io/admin-dashboard`
  - Node2: `http://node2.127.0.0.1.nip.io/admin-dashboard`
  - Node3: `http://node3.127.0.0.1.nip.io/admin-dashboard`

### 3. Full Secured Environment (Auth + Nodes)
```bash
# Start auth service first
docker-compose -f docker-compose.auth.yml up -d

# Then start nodes (they'll connect to auth)
docker-compose -f docker-compose.nodes.yml up
```
- Complete stack with auth protection
- API/WebSocket endpoints protected via ForwardAuth
- Dashboards remain publicly accessible
- Auth service accessible on each node subdomain:
  - `http://node1.127.0.0.1.nip.io/auth/login`
  - `http://node2.127.0.0.1.nip.io/auth/login`
  - `http://node3.127.0.0.1.nip.io/auth/login`

### 4. Configuration and Setup
```bash
# Initialize volumes and setup
docker-compose -f docker-compose.config.yml up --profile config

# Build applications (if needed)
docker-compose -f docker-compose.config.yml up --profile build

# Install applications to nodes
docker-compose -f docker-compose.config.yml up --profile install
```

### 5. Legacy Production Setup
```bash
docker-compose -f docker-compose.prod.yml up
```
- Single node + auth + proxy (all-in-one)
- For simple production deployments

## Environment Variables

- `CONTEXT_RECREATE=true` - Force recreation of node contexts
- `COMPOSE_PROFILES=node1|node2|node3` - Control which nodes to start
- `COMPOSE_PROJECT_NAME` - Override project name (default: calimero)
- `NODE1_URL`, `NODE2_URL`, `NODE3_URL` - Node endpoint URLs for configuration
- `APP_PATH` - Path to application WASM file for installation
- `NODE_NAME` - Target node name for application installation

## Profiles

### Configuration File Profiles:
- **config**: Run coordinator and setup services
- **build**: Run backend build service for applications
- **install**: Run application installer service

### Node File Profiles:
- **default**: All 3 nodes (node1, node2, node3)
- Individual nodes can be controlled via environment variables

## Service Dependencies

### Auth File (`docker-compose.auth.yml`):
- `auth` - Authentication service with `/auth/` and `/admin/` endpoints
- `proxy` - Traefik reverse proxy with subdomain routing

### Nodes File (`docker-compose.nodes.yml`):
- `node1`, `node2`, `node3` - Calimero nodes with subdomain routing
- Auth labels included but dormant when auth service not running
- Each node accessible at `nodeX.127.0.0.1.nip.io`

### Config File (`docker-compose.config.yml`):
- `init_volume` - Volume and permission initialization
- `backend_build` - WASM application compilation
- `coordinator` - Context and credential management
- `app_installer` - Application deployment to nodes

## Networks

Shared networks across all files:
- `web` - External facing (Traefik routing)
- `internal` - Inter-service communication

## Volumes

### Shared Volumes:
- `calimero_auth_node` - Shared node data and credentials
- `calimero_auth_data` - Auth service data
- `cargo-cache` - Rust build cache

## Development Workflow

### Basic Development:
1. **Initialize**: `docker-compose -f docker-compose.config.yml up --profile config`
2. **Start nodes**: `docker-compose -f docker-compose.nodes.yml up`
3. **Test without auth**: All endpoints publicly accessible

### Secured Development:
1. **Start auth**: `docker-compose -f docker-compose.auth.yml up -d`
2. **Start nodes**: `docker-compose -f docker-compose.nodes.yml up`
3. **Test with auth**: API/WebSocket protected, dashboards public

### Application Development:
1. **Build apps**: `docker-compose -f docker-compose.config.yml up --profile build`
2. **Install apps**: `docker-compose -f docker-compose.config.yml up --profile install`
3. **Reset contexts**: Set `CONTEXT_RECREATE=true` when needed

## Architecture Benefits

### 🔧 **Modular Design**
- Each concern separated into its own file
- Run only what you need
- Easy to maintain and understand

### 🚀 **Flexible Deployment**
- Standalone nodes for development
- Auth-protected nodes for production
- Configuration services run separately

### 🌐 **Subdomain Routing**
- Each node on its own subdomain using `nip.io`
- No `/etc/hosts` modifications needed
- Auth service accessible on all node subdomains

### 🔒 **Security Flexibility**
- Auth labels dormant when auth service not running
- Easy to switch between protected and open modes
- ForwardAuth middleware for API protection

## Production Deployment

### Recommended: Modular Approach
```bash
# Production with auth protection
docker-compose -f docker-compose.auth.yml up -d
docker-compose -f docker-compose.nodes.yml up -d
```

### Legacy: All-in-One
```bash
# Single node production
docker-compose -f docker-compose.prod.yml up -d
```

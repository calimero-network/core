# 🚀 Calimero Multi-Node Docker Setup

Complete guide for running Calimero multi-node setup with Docker.

## ⚡ Quick Start (Recommended)

Use the convenience script for the best experience:

```bash
# Simple standalone setup (3 nodes)
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend

# With authentication
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend -m with-auth

# Force recreate context
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend --recreate
```

## 📋 Script Options

```bash
./run-calimero.sh [OPTIONS]

Required:
  -w, --wasm PATH        Path to WASM file (.wasm)
  -f, --frontend PATH    Path to frontend source directory

Options:
  -m, --mode MODE        'standalone' (default) or 'with-auth'
  -r, --recreate         Force recreate context
  --remove-orphans       Remove orphaned containers
  -h, --help             Show help
```

## 📁 What You Need

1. **WASM File**: Your compiled application (`.wasm` file)
2. **Frontend**: Source directory with your frontend code

## 🌐 Access Your Applications

After startup, your applications will be available at:

**Standalone Mode:**
- **Node 1 API**: http://localhost:2528
- **Node 2 API**: http://localhost:2529  
- **Node 3 API**: http://localhost:2530
- **Node 1 Frontend**: http://localhost:5173
- **Node 2 Frontend**: http://localhost:5174  
- **Node 3 Frontend**: http://localhost:5175

**With-Auth Mode:**
- **Node 1**: http://node1.127.0.0.1.nip.io/admin-dashboard
- **Node 2**: http://node2.127.0.0.1.nip.io/admin-dashboard
- **Node 3**: http://node3.127.0.0.1.nip.io/admin-dashboard
- **Traefik Dashboard**: http://localhost:8080
- **Frontends**: Same as standalone mode (localhost:5173-5175)

## 🏗️ Architecture

This setup uses a modular Docker Compose architecture with 4 separate files:

- **`docker-compose.auth.yml`** - Auth service and Traefik proxy (core infrastructure)
- **`docker-compose.nodes.yml`** - Node services (1-3 nodes with subdomain routing)
- **`docker-compose.config.yml`** - Configuration services (coordinator, frontends)
- **`docker-compose.prod.yml`** - Legacy production setup (single node)

## 🛠️ Manual Setup

If you prefer manual control with Docker Compose:

### Standalone Mode (No Authentication)
```bash
# Set required paths
export WASM_FILE_PATH=/path/to/your/app.wasm
export FRONTEND_SOURCE_PATH=/path/to/your/frontend

# Run nodes + config
docker-compose -f docker-compose.nodes.yml -f docker-compose.config.yml up
```

### With Authentication
```bash
# Set required paths
export WASM_FILE_PATH=/path/to/your/app.wasm
export FRONTEND_SOURCE_PATH=/path/to/your/frontend

# Run auth + nodes + config (full stack)
docker-compose -f docker-compose.auth.yml -f docker-compose.nodes.yml -f docker-compose.config.yml up
```

### Individual Services

**Auth Service Only:**
```bash
docker-compose -f docker-compose.auth.yml up
```
- Runs auth service and Traefik proxy
- Auth UI: http://localhost/auth/
- Proxy dashboard: http://localhost:8080

**Nodes Only:**
```bash
docker-compose -f docker-compose.nodes.yml up
```
- Runs 3 nodes without auth protection
- All endpoints publicly accessible

## 🔧 Advanced Configuration

### Environment Variables

- **`WASM_FILE_PATH`** - Path to your WASM application file (required)
- **`FRONTEND_SOURCE_PATH`** - Path to your frontend source directory (required)
- **`CONTEXT_RECREATE`** - Force recreate context (`true`/`false`, default: `false`)
- **`COMPOSE_PROFILES`** - Service profiles (`complete`/`node1`/`node2`/`node3`, default: `complete`)
- **`COMPOSE_PROJECT_NAME`** - Docker project name (default: `calimero`)

### Running Specific Nodes

```bash
# Run only Node 1 + coordinator
COMPOSE_PROFILES=node1 docker-compose -f docker-compose.nodes.yml -f docker-compose.config.yml up

# Run Node 1 and Node 2
COMPOSE_PROFILES=node1,node2 docker-compose -f docker-compose.nodes.yml -f docker-compose.config.yml up
```

## 🔍 What Happens During Setup

1. **Volume Initialization**: Creates directory structure and sets permissions
2. **WASM Setup**: Copies your WASM file to the shared volume
3. **Application Installation**: Installs the WASM app to Node 1 using `meroctl`
4. **Context Creation**: Creates a new context with the installed application
5. **Node Configuration**: 
   - Generates identities for Node 2 and Node 3
   - Creates invitations from Node 1
   - Joins Node 2 and Node 3 to the context
6. **Credential Generation**: Creates environment files for all nodes
7. **Frontend Setup**: Builds and starts React frontends for each node

## 🆘 Troubleshooting

### "WASM_FILE_PATH is required"
Make sure you provide the path to your `.wasm` file:
```bash
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend
```

### "Frontend directory not found"
Ensure the frontend path points to a directory with your frontend source:
```bash
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend
```

### Start Fresh
To recreate everything from scratch:
```bash
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend --recreate
```

### Remove Old Containers
```bash
./run-calimero.sh -w /path/to/your/app.wasm -f /path/to/your/frontend --remove-orphans
```

### 502 Bad Gateway Errors
If you encounter 502 errors:
1. Stop all containers: `docker stop $(docker ps -q)`
2. Clean up: `docker system prune -f`
3. Restart with the script

### Check Container Status
```bash
# View running containers
docker ps

# Check specific service logs
docker-compose -f docker-compose.auth.yml logs auth
docker-compose -f docker-compose.nodes.yml logs node1
docker-compose -f docker-compose.config.yml logs coordinator
```

### Network Issues
All services use Docker networks:
- **`web`** - External network for Traefik routing
- **`internal`** - Internal network for service communication

If you have network conflicts, clean up with:
```bash
docker network prune
```

## 🔐 Authentication Flow

When using `with-auth` mode:

1. **Access Node Dashboard** → Redirects to auth login
2. **Auth Service** → Validates credentials
3. **Traefik Middleware** → Forwards auth headers
4. **Node API** → Receives authenticated requests

## 📊 Service Dependencies

```
auth ←── proxy (Traefik)
  ↓
nodes ←── coordinator ←── frontends
  ↑
  └── volumes ←── init_volume
```

## 🧹 Cleanup

Stop all services:
```bash
docker-compose -f docker-compose.auth.yml -f docker-compose.nodes.yml -f docker-compose.config.yml down
```

Remove volumes (⚠️ deletes all data):
```bash
docker-compose -f docker-compose.auth.yml -f docker-compose.nodes.yml -f docker-compose.config.yml down -v
```

Complete cleanup:
```bash
docker system prune -af
```

## 🛡️ Security Notes

- **Auth mode**: All admin endpoints protected by authentication
- **Standalone mode**: All endpoints publicly accessible (development only)
- **Networks**: Internal communication isolated from external access
- **Volumes**: Persistent data stored in Docker volumes

## 📈 Scaling

To add more nodes:
1. Add new node service in `docker-compose.nodes.yml`
2. Update coordinator logic in `docker-compose.config.yml`
3. Add Traefik routing labels
4. Update the script to support the new node

## ⚡ Performance Tips

- Use `--remove-orphans` to clean up old containers
- Set `CONTEXT_RECREATE=false` to reuse existing contexts
- Monitor container resource usage with `docker stats`
- Use Docker BuildKit for faster builds: `DOCKER_BUILDKIT=1`

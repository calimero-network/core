# 🚀 Calimero Multi-Node Quick Start

The easiest way to get started with Calimero multi-node setup.

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

## 🛠️ Manual Setup

If you prefer environment variables:

```bash
# Set required paths
export WASM_FILE_PATH=/path/to/your/app.wasm
export FRONTEND_SOURCE_PATH=/path/to/your/frontend

# Run standalone (nodes + config only)
docker-compose -f docker-compose.nodes.yml -f docker-compose.config.yml up

# Run with authentication (auth + nodes + config)
docker-compose -f docker-compose.auth.yml -f docker-compose.nodes.yml -f docker-compose.config.yml up

# Clean up orphaned containers
docker-compose -f docker-compose.nodes.yml -f docker-compose.config.yml up --remove-orphans
```

## 📁 What You Need

1. **WASM File**: Your compiled application (`.wasm` file)
2. **Frontend**: Source directory with your frontend code

## 🌐 Access Your Applications

After startup, your applications will be available at:

- **Node 1 Frontend**: http://localhost:5173
- **Node 2 Frontend**: http://localhost:5174  
- **Node 3 Frontend**: http://localhost:5175

## 🔧 Advanced Usage

For manual control, you can also use Docker Compose directly with environment variables:

```bash
WASM_FILE_PATH=/path/to/app.wasm FRONTEND_SOURCE_PATH=/path/to/frontend docker-compose -f docker-compose.nodes.yml -f docker-compose.config.yml up
```

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

## 📋 Available Script Options

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

# KV Store - Development Guide

## Quick Start

### Full Development Mode (with Local Registry)

```bash
pnpm dev:full
```

This starts:
- 🌐 **Local registry** (port 8082) - Stores and serves WASM files
- 🎨 **Vite dev server** (port 5173) - Frontend app
- 👀 **File watcher** - Auto-syncs WASM changes to registry

### What Happens When You Edit Logic?

1. Edit `logic/src/lib.rs`
2. Save file
3. Cargo rebuilds WASM (~5-10s)
4. Watcher detects `logic/res/kv_store.wasm` changed
5. **Auto-submits** manifest to local registry with `file://` URI
6. Registry CLI **copies** WASM to local storage
7. Registry **serves** WASM via HTTP
8. App refetches manifest from registry
9. **Test immediately** in browser at `http://localhost:5173`

---

## Development Modes

### Mode 1: Full Stack (Registry + App + Watcher)
```bash
pnpm dev:full
```
Use this for **package-based development** where the app fetches logic from the registry.

### Mode 2: Basic (App + Watcher)
```bash
pnpm dev
```
Use this for **frontend-only development** without registry integration.

### Mode 3: Merobox Network (App + Watcher + Merobox)
```bash
# Terminal 1: Start merobox network
pnpm network:bootstrap

# Terminal 2: Dev with merobox sync
pnpm dev:merobox
```
Use this for **network testing** with multiple nodes.

---

## Manual Commands

```bash
# Build logic
pnpm logic:build

# Sync to local registry
pnpm registry:sync

# Check registry status
pnpm registry:status

# Start registry manually
pnpm registry:start

# Reset registry (clean slate)
calimero-registry local reset --force

# Generate TypeScript client
pnpm app:generate-client
```

---

## App Configuration

The app is configured to use local registry in `app/src/App.tsx`:

```tsx
<CalimeroProvider
  packageName="network.calimero.kv-store"
  registryUrl="http://localhost:8082"
  // ... other props
>
  <YourApp />
</CalimeroProvider>
```

When the app starts, it will:
1. Fetch manifest from `http://localhost:8082/v1/apps/network.calimero.kv-store/0.2.5`
2. Get WASM URI from manifest
3. Download WASM from `http://localhost:8082/artifacts/network.calimero.kv-store/0.2.5/kv_store.wasm`

---

## File Structure

```
kv-store/
├── logic/                      # Rust smart contract
│   ├── src/lib.rs             # Contract code
│   ├── build.sh               # Build script
│   └── res/                   # Build output (gitignored)
│       ├── kv_store.wasm      # Compiled WASM
│       └── abi.json           # ABI schema
├── app/                       # React frontend
│   ├── src/
│   │   ├── App.tsx            # Main app (CalimeroProvider)
│   │   └── api/               # Generated TypeScript client
│   └── package.json
├── scripts/
│   ├── registry-sync.sh       # Syncs WASM to local registry
│   ├── on-res-change.mjs      # Watcher handler
│   └── sync-wasm.sh           # Syncs WASM to merobox nodes
├── manifest.json              # App manifest (for registry)
└── package.json
```

---

## Troubleshooting

### "Registry not running"

```bash
# Start manually:
pnpm registry:start

# Or check if it's already running:
pnpm registry:status
```

### "WASM file not found"

```bash
# Build it first:
pnpm logic:build
```

### "Port 8082 already in use"

```bash
# Use different port:
calimero-registry local start -p 8083

# Then update app to use new port:
# In app/src/App.tsx, change registryUrl to "http://localhost:8083"
```

### "Registry sync failed"

```bash
# Check registry health:
curl http://localhost:8082/healthz

# Check if manifest is valid:
cat manifest.json | jq '.'

# Manually sync with verbose output:
bash -x ./scripts/registry-sync.sh
```

### "Client code out of sync"

```bash
# Regenerate after logic changes:
pnpm app:generate-client

# This reads logic/res/abi.json and generates app/src/api/*
```

### "Old WASM cached"

```bash
# Reset registry (removes all stored apps/artifacts):
calimero-registry local reset --force

# Then rebuild and sync:
pnpm logic:build
pnpm registry:sync
```

---

## Advanced Usage

### Custom Registry URL

```bash
# Use different registry:
REGISTRY_URL=http://localhost:9000 pnpm dev:full
```

### Disable Auto-Sync

```bash
# Run watcher without registry sync:
SYNC_REGISTRY=false pnpm logic:watch
```

### Debug Mode

```bash
# Run registry with debug logging:
DEBUG=* pnpm registry:start

# Run sync script with verbose output:
bash -x ./scripts/registry-sync.sh
```

---

## How It Works

### Registry CLI Architecture

The `@calimero-network/registry-cli` tool provides:

1. **Local HTTP Server** (Fastify)
   - Implements V1 Registry API (`/v1/apps`)
   - Serves artifacts (`/artifacts/:appId/:version/:filename`)
   - Health checks (`/healthz`)

2. **Artifact Storage**
   - Stores manifests in-memory or on disk (`~/.calimero-registry/`)
   - Copies `file://` URIs to local storage
   - Serves via HTTP with proper CORS

3. **Automatic URI Conversion**
   - Receives manifest with `file:///path/to/app.wasm`
   - Copies WASM to `~/.calimero-registry/artifacts/app.id/version/`
   - Returns manifest with `http://localhost:8082/artifacts/...`

### Workflow Diagram

```
Developer                 Watcher                Registry CLI           Frontend App
    |                        |                         |                      |
    |--[Edit logic/src]----->|                         |                      |
    |                        |                         |                      |
    |--[Save file]---------->|                         |                      |
    |                        |                         |                      |
    |                        |--[Cargo build]--------->|                      |
    |                        |   (creates WASM)        |                      |
    |                        |                         |                      |
    |                        |--[POST manifest]------->|                      |
    |                        |   (file:// URI)         |                      |
    |                        |                         |                      |
    |                        |                    [Copy WASM]                 |
    |                        |                    [Store in ~/.calimero-...]  |
    |                        |                         |                      |
    |                        |<--[201 Created]---------|                      |
    |                        |   (app.id@version)      |                      |
    |                        |                         |                      |
    |                        |                         |<--[GET manifest]-----|
    |                        |                         |   (from app startup) |
    |                        |                         |                      |
    |                        |                         |--[Return manifest]-->|
    |                        |                         |   (http:// URI)      |
    |                        |                         |                      |
    |                        |                         |<--[GET /artifacts]---|
    |                        |                         |                      |
    |                        |                         |--[WASM bytes]------->|
    |                        |                         |                      |
    |                        |                         |                 [App runs]
```

---

## Next Steps

1. **Build the logic:** `pnpm logic:build`
2. **Start full dev:** `pnpm dev:full`
3. **Open browser:** http://localhost:5173
4. **Make changes** to `logic/src/lib.rs` and see them auto-sync!

Happy coding! 🚀

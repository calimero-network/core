# meroctl

Command-line interface for managing Calimero nodes, applications, contexts, and blobs. Provides a complete toolkit for development, deployment, and operations.

## Installation

```bash
# From source (requires Rust)
cargo install --path core/crates/meroctl

# Or build directly
cd core/crates/meroctl
cargo build --release
```

## Configuration

### Node Connection

Connect to a node using one of these methods:

```bash
# Using node alias (configured in ~/.calimero/config.toml)
meroctl --node node1 <command>

# Using direct API URL
meroctl --api http://localhost:2528 <command>
```

### Environment Variables

```bash
# Set default config directory
export CALIMERO_HOME=~/.calimero
```

## Commands

### Applications (`app`)

Manage WASM applications on nodes:

```bash
# List all applications
meroctl --node node1 app ls

# Get application details
meroctl --node node1 app get <app_id>

# Install application from WASM file
meroctl --node node1 app install \
  --path ./my-app.wasm \
  --application-id <app_id> \
  --context-id <context_id>

# Watch WASM file and auto-update contexts
meroctl --node node1 app watch <app_id> --path ./my-app.wasm

# Uninstall application
meroctl --node node1 app uninstall <app_id>

# List packages
meroctl --node node1 app list-packages

# List versions of a package
meroctl --node node1 app list-versions com.example.myapp

# Get latest version
meroctl --node node1 app get-latest-version com.example.myapp
```

### Contexts (`context`)

Manage application contexts:

```bash
# List all contexts
meroctl --node node1 context ls

# Create new context
meroctl --node node1 context create --application-id <app_id>

# Create context in dev mode (with watch)
meroctl --node node1 context create \
  --watch <path> \
  --context-id <context_id>

# Get context details
meroctl --node node1 context get <context_id>

# Join a context via invitation
meroctl --node node1 context join \
  --context-id <context_id> \
  --invitation <invitation_data>

# Join via open invitation
meroctl --node node1 context join-open --context-id <context_id>

# Invite member to context
meroctl --node node1 context invite \
  --context-id <context_id> \
  --identity <identity>

# Delete context
meroctl --node node1 context delete <context_id>

# Watch context for changes
meroctl --node node1 context watch <context_id>

# Manage context aliases
meroctl --node node1 context alias set <alias> <context_id>
meroctl --node node1 context alias ls
meroctl --node node1 context use <alias>
```

### Blobs (`blob`)

Manage binary blobs (files) on nodes:

```bash
# List all blobs
meroctl --node node1 blob ls

# Upload a blob from a file
meroctl --node node1 blob upload --file /path/to/file

# Upload and announce to context
meroctl --node node1 blob upload \
  --file /path/to/file \
  --context-id <context_id>

# Download a blob to a file
meroctl --node node1 blob download \
  --blob-id <blob_id> \
  --output /path/to/output

# Download with network discovery
meroctl --node node1 blob download \
  --blob-id <blob_id> \
  --output /path/to/output \
  --context-id <context_id>

# Get blob information
meroctl --node node1 blob info <blob_id>

# Delete a blob
meroctl --node node1 blob delete <blob_id>
```

### Calling Methods (`call`)

Execute methods on application contexts:

```bash
# Call a mutation method
meroctl --node node1 call <context_id> \
  --method set_item \
  --args '{"key": "foo", "value": "bar"}'

# Call a view method (read-only)
meroctl --node node1 call <context_id> \
  --method get_item \
  --args '{"key": "foo"}'
```

### Node Management (`node`)

Manage node connections:

```bash
# Add a local node
meroctl node add node1 /path/to/home

# Add a remote node
meroctl node add node2 http://public.node.com

# Add remote node with authentication
meroctl node add node3 http://private.node.com

# Set a node as active (default)
meroctl node use node1

# List all configured nodes
meroctl node ls

# Remove a node
meroctl node remove node1
```

### Peers (`peers`)

Query P2P network information:

```bash
# List connected peers
meroctl --node node1 peers ls

# Get peer details
meroctl --node node1 peers get <peer_id>
```

## Output Formats

Commands support multiple output formats:

```bash
# JSON output
meroctl --node node1 context ls --output json

# Table output (default)
meroctl --node node1 context ls --output table

# YAML output
meroctl --node node1 context ls --output yaml
```

## Authentication

`meroctl` supports authentication for private nodes:

- Automatically prompts for login when needed
- Supports NEAR wallet-based authentication
- Session caching for convenience
- Manual token management via `--access-token` and `--refresh-token` flags

## Examples

```bash
# Complete workflow: install app, create context, call method
meroctl --node node1 app install \
  --path ./kv-store.wasm \
  --application-id kv-store

meroctl --node node1 context create --application-id kv-store

meroctl --node node1 call <context_id> \
  --method set \
  --args '{"key": "hello", "value": "world"}'

meroctl --node node1 call <context_id> \
  --method get \
  --args '{"key": "hello"}'
```

## See Also

- [Calimero Documentation](https://docs.calimero.network) - Complete documentation
- [CLI Reference](../docs/tools-apis/meroctl-cli.md) - Detailed CLI documentation
- [Node Guide](../docs/operator-track/run-a-local-network.md) - Running nodes locally


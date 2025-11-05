# Calimero Application Development Guide

> **For LLMs assisting developers building applications on Calimero Network**

This guide is for developers building **distributed WASM applications** on Calimero, not for contributing to the core infrastructure. Focus on SDK usage, CRDT patterns, event handling, and client integration.

---

## What is Calimero?

Calimero is a **P2P framework for building distributed applications** with:
- ✅ **Automatic CRDT synchronization** - State syncs across nodes without manual merge code
- ✅ **Event-driven architecture** - Real-time notifications across the network
- ✅ **WASM runtime** - Write apps in Rust, compile to WebAssembly
- ✅ **Conflict-free data types** - Concurrent updates merge automatically
- ✅ **Local-first** - Apps work offline, sync when reconnected

**Key Architecture:**
1. Write WASM app using `calimero-sdk`
2. Build to `.wasm` file
3. Deploy to `merod` (node daemon)
4. Clients interact via JSON-RPC/WebSocket
5. State syncs automatically via P2P (Gossipsub + libp2p)

---

## Development Tools

### merod
**Node daemon** - Runs your WASM applications, handles sync, P2P networking, storage

```bash
# Start node
merod --config config.toml

# Or with environment variables
CALIMERO_NODE_PORT=2428 merod
```

### meroctl
**CLI tool** - Manage applications, contexts, and nodes

```bash
# Install an application
meroctl app install --path ./my-app.wasm --package com.example.myapp --version 1.0.0

# Create a context (runtime instance)
meroctl context create --application-id <app_id>

# List contexts
meroctl context ls

# Call a method
meroctl call --context-id <id> --method set --args '{"key":"foo","value":"bar"}'

# Watch for hot-reload during development
meroctl app watch <app_id> --path ./target/wasm32-unknown-unknown/release/my_app.wasm
```

### calimero-sdk
**Rust SDK** - Build WASM applications with automatic CRDT sync

### JSON-RPC API
**HTTP API** - Client applications call WASM methods via `POST /jsonrpc`

### WebSocket/SSE
**Real-time events** - Subscribe to state changes via `/ws` or `/sse`

---

## Building Your First App

### Setup

```bash
# Add WASM target
rustup target add wasm32-unknown-unknown

# Create new library
cargo new my-app --lib
```

### Cargo.toml

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
calimero-sdk = { path = "../../crates/sdk" }
calimero-storage = { path = "../../crates/storage" }
borsh = "1.3"
serde = { version = "1.0", features = ["derive"] }
thiserror = "1.0"

[profile.release]
opt-level = "z"
lto = true
```

### Minimal Application (src/lib.rs)

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshSerialize, BorshDeserialize};
use calimero_storage::collections::UnorderedMap;

#[app::state]
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MyApp {
    items: UnorderedMap<String, String>,
}

#[app::logic]
impl MyApp {
    #[app::init]
    pub fn init() -> MyApp {
        MyApp {
            items: UnorderedMap::new(),
        }
    }
    
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {} to value: {}", key, value);
        self.items.insert(key, value)?;
        Ok(())
    }
    
    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        self.items.get(key).map_err(Into::into)
    }
}
```

### Build

```bash
cargo build --target wasm32-unknown-unknown --release

# Output: target/wasm32-unknown-unknown/release/my_app.wasm
```

### Deploy and Test

```bash
# Install app
meroctl app install --path ./target/wasm32-unknown-unknown/release/my_app.wasm \
  --package com.example.myapp --version 1.0.0

# Create context
meroctl context create --application-id <app_id>

# Call method
meroctl call --context-id <context_id> --method set \
  --args '{"key":"hello","value":"world"}'

# Query
meroctl call --context-id <context_id> --method get \
  --args '{"key":"hello"}'
```

---

## ABI Generation and CRDT Transparency

### How CRDTs Appear in the ABI

**Important**: CRDT types are **transparent in the ABI** - they appear as their logical types, not their implementation types.

```rust
// In your Rust code
#[app::state]
pub struct MyApp {
    counter: Counter,                           // CRDT type
    items: UnorderedMap<String, String>,        // CRDT type
    name: LwwRegister<String>,                  // CRDT type
}
```

**Generated ABI shows:**
```json
{
  "types": {
    "MyApp": {
      "kind": "record",
      "fields": [
        {"name": "counter", "type": {"kind": "u64"}},
        {"name": "items", "type": {"kind": "map", "key": "string", "value": "string"}},
        {"name": "name", "type": {"kind": "string"}}
      ]
    }
  }
}
```

### CRDT Normalization Rules

| Rust Type | ABI Type | Rationale |
|-----------|----------|-----------|
| `Counter` | `u64` | Clients see it as a number |
| `UnorderedMap<K, V>` | `map<K, V>` | Clients see it as a map |
| `Vector<T>` | `list<T>` | Clients see it as a list |
| `UnorderedSet<T>` | `list<T>` | Clients see it as a list |
| `LwwRegister<T>` | `T` | Clients see the wrapped value |
| `ReplicatedGrowableArray` | `string` | Clients see it as text |

### Why This Design?

**✅ Correct approach:**
1. **Language Agnostic** - JS/Python/TS clients don't need Rust CRDT knowledge
2. **Implementation Detail** - CRDTs are *how* we achieve consistency, not *what* the data is
3. **Stable Interface** - Changing CRDT implementations doesn't break the ABI
4. **Clean API** - Clients work with familiar types (numbers, maps, strings)

**📝 What this means for you:**

```rust
// Method signature in Rust
pub fn get_count(&self) -> app::Result<u64> {
    Ok(self.counter.value()?)  // Counter::value() returns u64
}

// Clients call it and get u64 back
// They don't know (or care) that it's a G-Counter internally
```

### Extract ABI from WASM

After building, extract the ABI for client generation:

```bash
# Build WASM
cargo build --target wasm32-unknown-unknown --release

# Extract ABI (using calimero-abi tool)
calimero-abi extract target/wasm32-unknown-unknown/release/my_app.wasm \
  -o abi.json

# Now you can use abi.json to generate client SDKs
```

**ABI file structure:**
```json
{
  "schema_version": "wasm-abi/1",
  "types": { /* Custom types */ },
  "methods": [ /* All public methods */ ],
  "events": [ /* All events */ ],
  "state_root": "MyApp"  /* Root state type */
}
```

### Document CRDT Behavior

Since CRDTs are transparent in the ABI, **document their semantics separately**:

```rust
/// Get the total page view count across all nodes.
/// 
/// This is a distributed counter (G-Counter) that safely sums
/// concurrent increments from all nodes without conflicts.
/// The count only increases (never decreases).
pub fn get_page_views(&self) -> app::Result<u64> {
    Ok(self.page_views.value()?)
}

/// Set user profile. Last write wins if concurrent updates occur.
///
/// Uses Last-Write-Wins conflict resolution based on timestamps.
/// If two nodes update the same user simultaneously, the update
/// with the later timestamp wins.
pub fn set_user_profile(&mut self, id: String, profile: Profile) -> app::Result<()> {
    self.users.insert(id, profile.into())?;
    Ok(())
}
```

**Best Practice**: Add docstrings explaining CRDT semantics, especially for:
- Counters (grow-only, sum behavior)
- Maps (last-write-wins per key)
- Vectors (concurrent appends)
- Sets (union behavior)

---

## CRDT Collections (Critical!)

**Every piece of shared state MUST use CRDTs.** Regular Rust types lose concurrent updates.

### Available Collections

| Collection | Use Case | Merge Strategy | Can Nest? |
|------------|----------|----------------|-----------|
| `Counter` | Counters, metrics | Sum all increments | Leaf |
| `LwwRegister<T>` | Single values | Latest timestamp wins | Leaf |
| `UnorderedMap<K,V>` | Key-value storage | Per-entry merge | ✅ Yes |
| `Vector<T>` | Ordered lists | Element-wise merge | ✅ Yes |
| `UnorderedSet<T>` | Unique values | Union | Simple values |
| `ReplicatedGrowableArray` | Text editing | Character-level | Leaf |

### Counter (Distributed Counting)

```rust
use calimero_storage::collections::Counter;

#[app::state]
pub struct Analytics {
    page_views: Counter,
}

impl Analytics {
    pub fn track_view(&mut self) -> app::Result<()> {
        self.page_views.increment()?;  // All nodes' increments sum!
        Ok(())
    }
    
    pub fn total_views(&self) -> app::Result<u64> {
        Ok(self.page_views.value()?)  // Returns global sum
    }
}

// Node A increments: 5
// Node B increments: 7 (concurrent)
// After sync: total = 12 ✅
```

### UnorderedMap (Key-Value Storage)

```rust
use calimero_storage::collections::UnorderedMap;

#[app::state]
pub struct KvStore {
    items: UnorderedMap<String, String>,
}

impl KvStore {
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value)?;  // Last-Write-Wins per key
        Ok(())
    }
}

// Node A: items["key"] = "value_a" @ T1
// Node B: items["key"] = "value_b" @ T2 (concurrent)
// After sync: items["key"] = "value_b" (T2 > T1) ✅
```

### Nested CRDTs

```rust
use calimero_storage::collections::{UnorderedMap, Counter};

#[app::state]
pub struct Analytics {
    // Map of page paths to view counters
    page_views: UnorderedMap<String, Counter>,
}

impl Analytics {
    pub fn track_page(&mut self, page: String) -> app::Result<()> {
        let mut counter = self.page_views
            .get(&page)?
            .unwrap_or(Counter::new());
        counter.increment()?;
        self.page_views.insert(page, counter)?;
        Ok(())
    }
}

// Concurrent updates to DIFFERENT keys: both preserved ✅
// Concurrent increments to SAME counter: sum ✅
```

### LwwRegister (Last-Write-Wins)

```rust
use calimero_storage::collections::LwwRegister;

pub struct UserProfile {
    name: LwwRegister<String>,
    bio: LwwRegister<String>,
}

// Node A: profile.name = "Alice A" @ T1
// Node B: profile.name = "Alice B" @ T2
// After sync: profile.name = "Alice B" (latest timestamp) ✅
```

---

## Event System

### Define Events

```rust
#[app::event]
pub enum Event<'a> {
    ItemAdded { key: &'a str, value: &'a str },
    ItemRemoved { key: &'a str },
}
```

### Emit Events

```rust
// Without handler (notification only)
app::emit!(Event::ItemAdded { key: &key, value: &value });

// With handler (executes on receiving nodes)
app::emit!((
    Event::ItemAdded { key: &key, value: &value },
    "on_item_added"  // Handler method name
));
```

### Event Handlers

```rust
#[app::logic]
impl MyApp {
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key.clone(), value.clone())?;
        
        // Emit event WITH handler
        app::emit!((
            Event::ItemAdded { key: &key, value: &value },
            "log_addition"
        ));
        
        Ok(())
    }
    
    // Handler executes on RECEIVING nodes only (not author node)
    pub fn log_addition(&mut self, key: &str, value: &str) -> app::Result<()> {
        app::log!("Item added by another node: {} = {}", key, value);
        self.counter.increment()?;  // Track remote additions
        Ok(())
    }
}
```

### ⚠️ CRITICAL: Event Handler Requirements

**Handlers execute in PARALLEL and may run out of order!**

Your handlers **MUST** be:

#### 1. Commutative (Order-Independent)
```rust
// ✅ SAFE - Counter increment is commutative
pub fn handler_a(&mut self) { self.counter.increment(); }
pub fn handler_b(&mut self) { self.counter.increment(); }
// Result: counter = 2, regardless of order

// ❌ UNSAFE - Operations depend on order
pub fn create(&mut self, id: &str) { self.items.insert(id, "new"); }
pub fn update(&mut self, id: &str) {
    let item = self.items.get(id).expect("must exist");  // BREAKS if create() not run first!
}
```

#### 2. Independent (No Shared State)
```rust
// ✅ SAFE - Each handler uses unique key
pub fn handler_a(&mut self, user: &str) {
    self.counters.insert(format!("a_{}", user), Counter::new());
}

// ❌ UNSAFE - Both modify same key
pub fn handler_a(&mut self) {
    self.shared.insert("count", "1");  // RACE CONDITION!
}
pub fn handler_b(&mut self) {
    self.shared.insert("count", "2");  // RACE CONDITION!
}
```

#### 3. Idempotent (Safe to Retry)
```rust
// ✅ SAFE - CRDT operations are idempotent
pub fn handler(&mut self) {
    self.counter.increment();  // Safe to call multiple times
}

// ❌ UNSAFE - Side effects not idempotent
pub fn handler(&mut self, amount: u64) {
    external_payment_api::charge(amount);  // DANGER: May charge twice!
}
```

#### 4. Pure (No External Side Effects)
```rust
// ✅ SAFE - Only modifies CRDT state
pub fn handler(&mut self, item: &str) {
    self.items.insert(item.to_owned(), "processed".to_owned());
    app::log!("Handler called");  // Logging is fine
}

// ❌ UNSAFE - External side effects
pub fn handler(&mut self, email: &str) {
    http_client::post("/notify", email);  // DANGER: Not deterministic!
}
```

**Why author nodes skip handlers:** Prevents infinite loops where a node executes its own handler, which emits an event, which triggers the handler again, etc.

---

## Blob API (File Storage)

### Upload Flow

**Client-side:**
```typescript
// 1. Upload binary to blob storage
const blobResponse = await blobClient.uploadBlob(file);
const blobId = blobResponse.data.blobId;  // Base58-encoded

// 2. Call contract with blob ID and metadata
await contractApi.upload_file(
  file.name,
  blobId,
  file.size,
  file.type
);
```

**WASM Contract:**
```rust
use calimero_sdk::{app, env};

#[derive(BorshSerialize, BorshDeserialize)]
pub struct FileRecord {
    pub id: String,
    pub name: String,
    pub blob_id: [u8; 32],  // Binary blob ID
    pub size: u64,
    pub mime_type: String,
}

#[app::logic]
impl FileShare {
    pub fn upload_file(
        &mut self,
        name: String,
        blob_id_str: String,  // Base58-encoded
        size: u64,
        mime_type: String,
    ) -> app::Result<String> {
        // Parse blob ID from base58
        let blob_id = bs58::decode(&blob_id_str)
            .into_vec()
            .map_err(|e| app::Error::msg(format!("Invalid blob ID: {}", e)))?;
        let blob_id: [u8; 32] = blob_id.try_into()
            .map_err(|_| app::Error::msg("Blob ID must be 32 bytes"))?;
        
        // CRITICAL: Announce blob to network!
        let context = env::context_id();
        if !env::blob_announce_to_context(&blob_id, &context) {
            app::log!("Warning: Failed to announce blob");
        }
        
        // Store metadata
        let file_id = format!("file_{}", self.counter);
        self.counter += 1;
        
        let record = FileRecord {
            id: file_id.clone(),
            name,
            blob_id,
            size,
            mime_type,
        };
        
        self.files.insert(file_id.clone(), record)?;
        Ok(file_id)
    }
    
    pub fn get_blob_id(&self, file_id: &str) -> app::Result<String> {
        let record = self.files.get(file_id)?
            .ok_or_else(|| app::Error::msg("File not found"))?;
        
        // Return base58-encoded for client
        Ok(bs58::encode(&record.blob_id).into_string())
    }
}
```

### Download Flow

**Client-side:**
```typescript
// 1. Get blob ID from contract
const blobId = await contractApi.get_blob_id(fileId);

// 2. Download from network
const blobData = await blobClient.downloadBlob(blobId, contextId);

// 3. Use the blob
const url = URL.createObjectURL(blobData);
```

**Key Points:**
- Blob IDs are 32 bytes internally, Base58-encoded for JSON
- `env::blob_announce_to_context()` is CRITICAL - makes blob discoverable
- Blobs are content-addressed, metadata stored in contract state
- Only nodes in the same context can discover/download blobs

---

## JSON-RPC API (Client Integration)

### Endpoint

`POST /jsonrpc`

### Execute Transaction

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "execute",
  "params": {
    "context_id": "context_id_here",
    "executor_public_key": "public_key_here",
    "method": "set",
    "args_json": {
      "key": "hello",
      "value": "world"
    }
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "returns": null  // or method return value
  }
}
```

### Real-Time Events

#### WebSocket (`/ws`)

```javascript
const ws = new WebSocket('ws://localhost:2428/ws');

ws.onopen = () => {
  // Subscribe to context
  ws.send(JSON.stringify({
    Subscribe: {
      context_id: "context_id_here"
    }
  }));
};

ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  // Handle events
};
```

#### Server-Sent Events (`/sse`)

```javascript
const sse = new EventSource('http://localhost:2428/sse');

sse.addEventListener('connect', (event) => {
  const { connection_id } = JSON.parse(event.data);
  
  // Subscribe to context
  fetch('/sse/subscription', {
    method: 'POST',
    body: JSON.stringify({
      connection_id,
      subscribe: ["context_id_here"]
    })
  });
});

sse.addEventListener('message', (event) => {
  // Handle events
});
```

---

## Available Macros Reference

### State and Logic Macros

#### `#[app::state]`
Defines the application's persistent state (synchronized across all nodes).

```rust
#[app::state]
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MyApp {
    items: UnorderedMap<String, String>,
}

// With events
#[app::state(emits = for<'a> Event<'a>)]
pub struct MyApp { /* ... */ }
```

#### `#[app::logic]`
Marks the implementation block containing your application logic.

```rust
#[app::logic]
impl MyApp {
    // Your methods here
}
```

#### `#[app::init]`
Marks the constructor/initialization method (called once when context is created).

```rust
#[app::logic]
impl MyApp {
    #[app::init]
    pub fn init() -> MyApp {
        MyApp {
            items: UnorderedMap::new(),
        }
    }
}
```

#### `#[app::event]`
Defines events that can be emitted and handled.

```rust
#[app::event]
pub enum Event<'a> {
    ItemAdded { key: &'a str, value: &'a str },
    ItemRemoved { key: &'a str },
}
```

#### `#[app::private]`
**Defines private (node-local) storage that is NOT synchronized.**

Use this for:
- Secrets and sensitive data
- Node-specific configuration
- Caching/temporary data
- Data that should remain local to each node

```rust
#[derive(BorshSerialize, BorshDeserialize, Debug, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private]
pub struct Secrets {
    api_keys: UnorderedMap<String, String>,
    local_cache: UnorderedMap<String, String>,
}

// Auto-generates these methods:
// - Secrets::private_handle()
// - Secrets::private_load()
// - Secrets::private_load_or_default()
// - Secrets::private_load_or_init_with(f)
```

**Using private storage:**

```rust
#[app::logic]
impl MyApp {
    pub fn store_secret(&mut self, key: String, secret: String) -> app::Result<()> {
        // Load private storage (or initialize with default)
        let mut secrets = Secrets::private_load_or_default()?;
        
        // Get mutable reference
        let mut secrets_mut = secrets.as_mut();
        
        // Modify (auto-saved on drop)
        secrets_mut.api_keys.insert(key, secret)?;
        
        Ok(())
        // secrets_mut dropped here -> automatically saved!
    }
    
    pub fn get_secret(&self, key: &str) -> app::Result<Option<String>> {
        let secrets = Secrets::private_load_or_default()?;
        secrets.api_keys.get(key).map_err(Into::into)
    }
}
```

**⚠️ Important: Private storage is LOCAL ONLY**
- NOT synchronized across nodes
- Each node has its own copy
- Perfect for secrets, but NOT for shared application state
- Use regular `#[app::state]` with CRDTs for synchronized data

### Event Macros

#### `app::emit!(event)`
Emits an event without a handler (notification only).

```rust
app::emit!(Event::ItemAdded { key: &key, value: &value });
```

#### `app::emit!((event, "handler"))`
Emits an event WITH a handler (executes on receiving nodes).

```rust
app::emit!((
    Event::ItemAdded { key: &key, value: &value },
    "on_item_added"  // Handler method name
));
```

### Error Handling Macros

#### `app::err!(message)`
Creates an error result.

```rust
pub fn get(&self, key: &str) -> app::Result<String> {
    if !self.items.contains(key)? {
        return app::err!("Key not found: {}", key);
    }
    // ...
}
```

#### `app::bail!(message)`
Returns early with an error (like `return Err(...)`).

```rust
pub fn get(&self, key: &str) -> app::Result<String> {
    if !self.items.contains(key)? {
        app::bail!("Key not found: {}", key);
    }
    // Continues only if key exists
    Ok(self.items.get(key)?.unwrap())
}
```

### Logging Macro

#### `app::log!(message)`
Logs a message (visible in merod output).

```rust
app::log!("Setting key: {}", key);
app::log!("User {} performed action", user_id);
```

### Lifecycle Macros

#### `#[app::init]`
Constructor - called once when context is created.

```rust
#[app::init]
pub fn init() -> MyApp {
    MyApp { items: UnorderedMap::new() }
}
```

#### `#[app::destroy]`
Destructor - called when context is destroyed (cleanup).

```rust
#[app::destroy]
pub fn destroy(&mut self) {
    app::log!("Cleaning up resources");
    // Cleanup code here
}
```

---

## Environment Functions

```rust
use calimero_sdk::env;

// Get current executor (who called this method)
let executor = env::executor_id();  // [u8; 32]

// Get current context
let context = env::context_id();  // [u8; 32]

// Get current timestamp
let now = env::time_now();  // u64 nanoseconds

// Logging (shows in merod output)
env::log("Hello from WASM");
app::log!("Formatted: {}", value);

// Blob operations
env::blob_announce_to_context(&blob_id, &context_id);  // Make blob discoverable

// Storage operations (usually handled by macros)
env::storage_read(key);   // Read from storage
env::storage_write(key, data);  // Write to storage
```

---

## Testing Your Application

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_set_and_get() {
        let mut app = MyApp::init();
        
        app.set("key".to_owned(), "value".to_owned()).unwrap();
        
        let result = app.get("key").unwrap();
        assert_eq!(result, Some("value".to_owned()));
    }
    
    #[test]
    fn test_counter_increments() {
        let mut app = MyApp::init();
        
        app.counter.increment().unwrap();
        app.counter.increment().unwrap();
        
        assert_eq!(app.counter.value().unwrap(), 2);
    }
}
```

### E2E Tests

Create test file in `e2e-tests/config/protocols/near/my-app-test.json`:

```json
{
  "protocol": "near",
  "steps": [
    {
      "action": "call",
      "node": "inviter",
      "method": "set",
      "args": {"key": "test", "value": "hello"},
      "expectedResultJson": null
    },
    {
      "action": "wait",
      "durationMs": 3000
    },
    {
      "action": "call",
      "node": "invitee",
      "method": "get",
      "args": {"key": "test"},
      "expectedResultJson": "hello"
    }
  ]
}
```

Run tests:
```bash
cd e2e-tests
cargo run -- \
  --merod-binary ../target/release/merod \
  --meroctl-binary ../target/release/meroctl \
  --protocols near
```

---

## Multi-Node Development

### Start Two Nodes

```bash
# Terminal 1: Coordinator node
merod --node-type coordinator --port 2428

# Terminal 2: Peer node
merod --node-type peer --port 2429 \
  --swarm-addrs /ip4/127.0.0.1/tcp/2428
```

### Create and Join Context

```bash
# On node 1: Create context
meroctl --node-name node1 context create --application-id <app_id>

# On node 2: Join context
meroctl --node-name node2 context join <context_id>

# Call method on node 1
meroctl --node-name node1 call --context-id <id> --method set \
  --args '{"key":"foo","value":"bar"}'

# Wait for sync (~1-3 seconds)
sleep 3

# Verify on node 2
meroctl --node-name node2 call --context-id <id> --method get \
  --args '{"key":"foo"}'
# Should return "bar" ✅
```

---

## Common Pitfalls

### ❌ Don't: Use Regular Types for Shared State

```rust
#[app::state]
struct App {
    counter: u64,  // WRONG - loses concurrent updates!
}

impl App {
    pub fn increment(&mut self) {
        self.counter += 1;  // Concurrent increments will conflict
    }
}
```

### ✅ Do: Use CRDTs

```rust
#[app::state]
struct App {
    counter: Counter,  // RIGHT - sums all increments
}

impl App {
    pub fn increment(&mut self) -> app::Result<()> {
        self.counter.increment()?;  // All nodes' increments sum correctly
        Ok(())
    }
}
```

### ❌ Don't: Make Handlers Order-Dependent

```rust
pub fn create_user(&mut self, id: &str) -> app::Result<()> {
    self.users.insert(id.to_owned(), User::default())?;
    app::emit!((Event::UserCreated { id }, "send_welcome_email"));
    Ok(())
}

pub fn send_welcome_email(&mut self, id: &str) -> app::Result<()> {
    let user = self.users.get(id)?.expect("user must exist");  // WRONG - might run before create!
    // ...
}
```

### ✅ Do: Make Handlers Independent

```rust
pub fn create_user(&mut self, id: &str) -> app::Result<()> {
    self.users.insert(id.to_owned(), User::default())?;
    app::emit!((Event::UserCreated { id }, "increment_user_count"));
    Ok(())
}

pub fn increment_user_count(&mut self, _id: &str) -> app::Result<()> {
    self.user_count.increment()?;  // Safe - no dependencies
    Ok(())
}
```

### ❌ Don't: Forget to Announce Blobs

```rust
pub fn upload_file(&mut self, blob_id: [u8; 32]) -> app::Result<()> {
    self.files.insert(generate_id(), blob_id)?;
    Ok(())
    // WRONG - other nodes can't discover this blob!
}
```

### ✅ Do: Always Announce Blobs

```rust
pub fn upload_file(&mut self, blob_id: [u8; 32]) -> app::Result<()> {
    let context = env::context_id();
    env::blob_announce_to_context(&blob_id, &context);  // RIGHT - makes blob discoverable
    self.files.insert(generate_id(), blob_id)?;
    Ok(())
}
```

### ❌ Don't: Use Panics or Unwraps in Production

```rust
pub fn get(&self, key: &str) -> app::Result<String> {
    Ok(self.items.get(key)?.expect("key not found"))  // WRONG - panics!
}
```

### ✅ Do: Return Errors Gracefully

```rust
pub fn get(&self, key: &str) -> app::Result<String> {
    let Some(value) = self.items.get(key)? else {
        app::bail!("Key not found: {}", key);
    };
    Ok(value)
}
```

---

## Key Concepts Glossary

- **Context** - Runtime instance of your app (like a "room" or "workspace"). Multiple contexts can run the same app with separate state.
- **Delta** - A CRDT change that gets synced between nodes. Contains the actual state updates.
- **DAG** - Directed Acyclic Graph tracking causal order of deltas. Ensures correct merge order.
- **Gossipsub** - P2P broadcast protocol for delta propagation. Typical latency: 100-200ms.
- **LWW** - Last-Write-Wins. Timestamp-based conflict resolution (newer timestamp wins).
- **G-Counter** - Grow-only counter. Sums increments from all nodes without conflicts.
- **CRDT** - Conflict-free Replicated Data Type. Data structures that merge automatically without conflicts.
- **Executor** - The identity calling a method (usually a user's public key).
- **Author Node** - The node that created a delta. Author nodes skip their own event handlers.

---

## Private Storage Example (Secret Game)

A practical example combining synchronized state with private (node-local) storage:

```rust
use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use sha2::{Digest, Sha256};

// PUBLIC state (synchronized across all nodes)
#[app::state(emits = Event)]
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct SecretGame {
    // Public hash of secrets (synchronized)
    games: UnorderedMap<String, LwwRegister<String>>,
}

// PRIVATE state (local to each node, NOT synchronized)
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private]
pub struct Secrets {
    // Actual secrets stored locally only
    secrets: UnorderedMap<String, String>,
}

#[app::logic]
impl SecretGame {
    #[app::init]
    pub fn init() -> SecretGame {
        SecretGame {
            games: UnorderedMap::new(),
        }
    }
    
    // Store secret privately, publish hash publicly
    pub fn add_secret(&mut self, game_id: String, secret: String) -> app::Result<()> {
        // Store actual secret in PRIVATE storage (local only)
        let mut secrets = Secrets::private_load_or_default()?;
        let mut secrets_mut = secrets.as_mut();
        secrets_mut.secrets.insert(game_id.clone(), secret.clone())?;
        
        // Store hash in PUBLIC state (synchronized)
        let hash = Sha256::digest(secret.as_bytes());
        let hash_hex = hex::encode(hash);
        self.games.insert(game_id, hash_hex.into())?;
        
        Ok(())
    }
    
    // Verify guess against public hash
    pub fn verify_guess(&self, game_id: &str, guess: String) -> app::Result<bool> {
        let public_hash = self.games.get(game_id)?
            .ok_or_else(|| app::Error::msg("Game not found"))?;
        
        let guess_hash = hex::encode(Sha256::digest(guess.as_bytes()));
        Ok(guess_hash == public_hash.get())
    }
    
    // Get YOUR local secrets (not synced with other nodes)
    pub fn my_secrets(&self) -> app::Result<Vec<(String, String)>> {
        let secrets = Secrets::private_load_or_default()?;
        Ok(secrets.secrets.entries()?.collect())
    }
}
```

**Why use private storage here?**
- Actual secrets stay on the node that created them
- Other nodes only see hashes (can verify guesses)
- Each node can have different secrets for the same game
- Perfect for node-specific data like API keys, local caches, etc.

---

## Example Applications to Study

Located in `apps/` directory:

- **`kv-store/`** - Basic key-value store with CRDT map
- **`kv-store-with-handlers/`** - Events and handler patterns
- **`blobs/`** - File storage with blob API integration
- **`collaborative-editor/`** - Real-time text editing with RGA
- **`private_data/`** - **Private storage patterns** (secrets, node-local data)
- **`xcall-example/`** - Cross-chain integration

---

## Performance Characteristics

- **Sync Latency**: 100-200ms (Gossipsub broadcast)
- **Throughput**: 100-1000 deltas/sec per context
- **Memory**: ~10MB per context (1000 deltas)
- **Merge Overhead**: < 1% of operations, 1-2ms when needed
- **Network dominates**: Merge time negligible vs network latency

---

## Development Best Practices

1. **Always use CRDTs for shared state** - Never use regular Rust types
2. **Keep handlers simple and independent** - Avoid complex logic with dependencies
3. **Test with multiple nodes** - Single-node tests miss sync issues
4. **Log liberally during development** - Use `app::log!()` to debug sync issues
5. **Handle errors gracefully** - Return `app::Result`, don't panic
6. **Announce blobs immediately** - Don't forget `env::blob_announce_to_context()`
7. **Use hot-reload during development** - `meroctl app watch` for fast iteration
8. **Monitor event handlers** - Ensure they're commutative and idempotent

---

## Additional Resources

- **SDK Documentation**: `crates/sdk/README.md` - Complete API reference
- **Storage Guide**: `crates/storage/README.md` - CRDT collections in depth
- **Server API**: `crates/server/README.md` - JSON-RPC and WebSocket details
- **Node Integration**: `crates/node/readme/integration-guide.md` - Advanced node usage
- **Main README**: `README.mdx` - Project overview and architecture

---

## Quick Reference Card

```rust
// ============ State Definition ============
#[app::state]  // Synchronized across nodes
struct MyApp { items: UnorderedMap<String, String> }

#[app::state(emits = Event)]  // With events
struct MyApp { /* ... */ }

#[app::private]  // Local to each node (NOT synchronized)
struct Secrets { api_keys: UnorderedMap<String, String> }

// ============ Logic Implementation ============
#[app::logic]
impl MyApp {
    #[app::init]  // Constructor
    pub fn init() -> MyApp { /* ... */ }
    
    #[app::destroy]  // Destructor (cleanup)
    pub fn destroy(&mut self) { /* ... */ }
    
    pub fn method(&mut self) -> app::Result<()> { /* ... */ }
}

// ============ Events ============
#[app::event]
enum Event { Created { id: String } }

app::emit!(Event::Created { id });  // Without handler
app::emit!((Event::Created { id }, "handler_name"));  // With handler

// ============ Error Handling ============
app::err!("error: {}", msg)    // Create error
app::bail!("error: {}", msg)   // Return early with error

// ============ Logging ============
app::log!("formatted {}", value)  // Log message
env::log("msg")                    // Low-level log

// ============ Environment ============
env::executor_id()  // Current executor [u8; 32]
env::context_id()   // Current context [u8; 32]
env::time_now()     // Timestamp (u64)
env::storage_read(key)   // Direct storage read
env::storage_write(key, data)  // Direct storage write

// ============ Private Storage ============
let mut secrets = Secrets::private_load_or_default()?;
let mut secrets_mut = secrets.as_mut();
secrets_mut.api_keys.insert(key, value)?;
// Auto-saved on drop!

// ============ CRDT Collections ============
Counter::new()                      // Distributed counter (G-Counter)
LwwRegister::new(value)             // Last-write-wins register
UnorderedMap::new()                 // CRDT map (per-key LWW)
Vector::new()                       // CRDT vector (ordered list)
UnorderedSet::new()                 // CRDT set (union merge)
ReplicatedGrowableArray::new()     // Text CRDT (character-level)

// ============ Blob API ============
env::blob_announce_to_context(&blob_id, &context_id);  // Make discoverable
```

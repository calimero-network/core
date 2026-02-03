# Node Integration Guide

How to integrate Calimero Node into your application.

---

## Overview

The node layer provides **ready-to-use** synchronization and event handling. You typically don't integrate it directly—instead, you use it via `merod` (the node daemon) or embed it in your server.

---

## Using merod (Recommended)

### Start Node Daemon

```bash
# Start node with default config
merod --config config.toml

# Or with environment variables
CALIMERO_NODE_PORT=2428 \
CALIMERO_SYNC_INTERVAL=5 \
merod
```

### Configuration File

```toml
# config.toml
[node]
port = 2428
host = "0.0.0.0"

[sync]
interval = 5
frequency = 10
timeout = 30
max_concurrent = 30

[storage]
path = "./data"
```

### Interact via HTTP API

```bash
# Create context
curl -X POST http://localhost:2428/contexts \
  -H "Content-Type: application/json" \
  -d '{"protocol": "near"}'

# Execute transaction
curl -X POST http://localhost:2428/execute/:context_id \
  -H "Content-Type: application/json" \
  -d '{
    "method": "add_item",
    "args": {"name": "Widget"}
  }'

# Query state
curl http://localhost:2428/query/:context_id?method=get_items
```

---

## Embedding in Server

### Setup

```toml
[dependencies]
calimero-node = { path = "../node" }
calimero-context = { path = "../context" }
tokio = { version = "1.0", features = ["full"] }
actix = "0.13"
```

### Start Node Manager

```rust
use calimero_node::{start, NodeConfig};
use calimero_context::ContextClient;
use calimero_node_primitives::NodeClient;

#[tokio::main]
async fn main() -> Result<()> {
    // Create clients
    let context_client = ContextClient::new(/* ... */);
    let node_client = NodeClient::new(/* ... */);
    
    // Create config
    let config = NodeConfig {
        sync_config: Default::default(),
        ..Default::default()
    };
    
    // Start node
    let node_handle = start(
        blobstore,
        sync_manager,
        context_client,
        node_client,
    ).await?;
    
    // Node running in background
    println!("Node started");
    
    // Keep running
    tokio::signal::ctrl_c().await?;
    
    Ok(())
}
```

---

## Creating Contexts

### Via Node Client

```rust
use calimero_primitives::context::ContextId;

// Create context
let context_id = node_client.create_context(protocol).await?;

// Subscribe to context (for sync)
node_client.subscribe(&context_id).await?;

// Node will automatically:
// - Create DeltaStore for context
// - Subscribe to gossipsub topic
// - Start periodic sync
```

---

## Executing Transactions

### Via Context Client

```rust
// Execute method
let outcome = context_client.execute(
    &context_id,
    &executor_id,
    "add_item",
    borsh::to_vec(&args)?,
    vec![],  // attachments
    None,    // no parent
).await?;

// Node automatically:
// - Runs WASM
// - Creates CausalDelta
// - Broadcasts via gossipsub
// - Updates DAG heads
```

---

## Handling Events

### Define Event Handler

```rust
#[app::event]
pub enum MyEvent {
    ItemAdded { name: String },
}

#[app::event_handler]
impl MyApp {
    pub fn on_item_added(&mut self, event: MyEvent) {
        // Handler runs on receiving nodes
        self.counter.increment();
    }
}
```

### Node Automatically

- Receives delta via gossipsub
- Checks if event has handler
- Skips if author node
- Executes handler in WASM
- Emits to WebSocket clients

---

## Monitoring

### Health Check

```rust
use actix_web::{web, App, HttpServer};

async fn health() -> Result<String> {
    let stats = node.get_stats().await?;
    Ok(format!("Alive: {} contexts", stats.context_count))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new()
            .route("/health", web::get().to(health))
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
```

### Metrics

```rust
// Export Prometheus metrics
use prometheus::{Encoder, TextEncoder};

async fn metrics() -> Result<String> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer)?;
    
    Ok(String::from_utf8(buffer)?)
}
```

---

## Testing

### Integration Test

```rust
#[tokio::test]
async fn test_node_integration() {
    // Setup
    let node = start_test_node().await;
    let context_id = create_test_context(&node).await;
    
    // Execute transaction
    let result = node.execute(
        &context_id,
        "add_item",
        &args,
    ).await.unwrap();
    
    assert!(result.success);
    
    // Wait for propagation
    tokio::time::sleep(Duration::from_millis(200)).await;
    
    // Verify on other node
    let other_node = start_test_node().await;
    other_node.subscribe(&context_id).await.unwrap();
    
    // Sync should happen automatically
    tokio::time::sleep(Duration::from_secs(1)).await;
    
    // Verify state
    let state = other_node.query(&context_id, "get_items").await.unwrap();
    assert_eq!(state.items.len(), 1);
}
```

---

## Best Practices

### 1. Use Presets

```rust
// Don't configure everything manually
let config = NodeConfig::production();  // Use preset

// Only override what you need
let config = NodeConfig {
    sync_config: SyncConfig {
        interval: Duration::from_secs(2),
        ..Default::default()
    },
    ..NodeConfig::production()
};
```

### 2. Monitor Pending Deltas

```rust
// Check periodically
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        
        for (context_id, delta_store) in node.delta_stores.iter() {
            let stats = delta_store.pending_stats().await;
            if stats.count > 100 {
                warn!("Context {} has {} pending deltas", context_id, stats.count);
            }
        }
    }
});
```

### 3. Handle Errors Gracefully

```rust
match node.execute(&context_id, method, args).await {
    Ok(outcome) => {
        // Success
    }
    Err(e) => {
        error!("Execution failed: {}", e);
        // Don't panic - log and continue
    }
}
```

### 4. Clean Up Resources

```rust
// On context delete
node.unsubscribe(&context_id).await?;
node.delete_delta_store(&context_id).await?;

// Periodic GC
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    loop {
        interval.tick().await;
        node.gc().await?;
    }
});
```

---

## See Also

- [Architecture](architecture.md) - How node works
- [Sync Configuration](sync-configuration.md) - How to configure
- [Event Handling](event-handling.md) - Event system
- [Performance](performance.md) - Performance tuning


# Node Troubleshooting Guide

Common issues and solutions for Calimero Node.

---

## Nodes Not Syncing

### Symptom
Nodes have different states despite being in same context.

### Diagnosis
```bash
# Check pending deltas
curl http://localhost:2428/debug/pending

# Check DAG heads
curl http://localhost:2428/debug/heads/:context_id

# Check sync stats
curl http://localhost:2428/debug/sync_stats
```

### Common Causes

**1. Network Partition**
- Nodes can't reach each other
- Gossipsub not working
- Firewall blocking P2P

**Solution**:
```bash
# Check connectivity
nc -zv <peer_ip> <peer_port>

# Check gossipsub subscription
curl http://localhost:2428/debug/subscriptions
```

**2. Missing Parent Deltas**
- Deltas stuck in pending
- Parents never requested

**Solution**:
```rust
// Check missing parents
let missing = dag.get_missing_parents();
for parent_id in missing {
    request_from_peer(parent_id).await?;
}
```

**3. Sync Interval Too Long**
- Periodic sync not frequent enough
- Broadcasts failing silently

**Solution**:
```rust
// Reduce sync interval
let config = SyncConfig {
    interval: Duration::from_secs(2),
    frequency: Duration::from_secs(5),
    ..Default::default()
};
```

---

## Events Not Executing

### Symptom
Events emitted but handlers don't run.

### Diagnosis
```bash
# Enable debug logging
RUST_LOG=calimero_node=debug cargo run

# Look for:
# - "Event emitted"
# - "Executing event handler"
# - "Handler execution completed"
```

### Common Causes

**1. Author Node**
- You're the node that created the delta
- Authors don't execute their own handlers

**Expected Behavior**:
```rust
// Node A creates delta with event
app::emit!(ItemAdded { name });

// Node A: Handler NOT executed (author)
// Node B, C, D: Handler executed (receivers)
```

**2. Handler Not Registered**
- Handler name doesn't match event
- Missing `#[app::event_handler]` macro

**Solution**:
```rust
// ✅ Correct naming
#[app::event]
pub enum MyEvent {
    ItemAdded { name: String },  // Event name
}

#[app::event_handler]
impl MyApp {
    pub fn on_item_added(&mut self, event: MyEvent) {  // "on_" + snake_case
        // Handler code
    }
}
```

**3. Delta Not Applied**
- Delta still pending
- Handlers buffered until delta applied

**Solution**:
```bash
# Check pending count
curl http://localhost:2428/debug/pending/:context_id

# If high, request missing parents or trigger sync
```

---

## Memory Growing

### Symptom
Node memory usage increases over time.

### Diagnosis
```bash
# Monitor memory
top -p $(pgrep merod)

# Check DAG stats
curl http://localhost:2428/debug/dag_stats
```

### Common Causes

**1. Pending Deltas Not Cleaned**
- Stale deltas accumulating
- No cleanup running

**Solution**:
```rust
// Cleanup runs every 60s automatically
// Check logs for:
warn!("Evicted {} stale pending deltas", evicted);

// If not running, check timer started
```

**2. Blob Cache Growing**
- Blobs not evicted
- Cache limits too high

**Solution**:
```rust
// Check blob cache size
let stats = node.state.blob_cache.len();

// Eviction runs every 5 min
// Limits: 100 blobs, 500 MB, 5 min age
```

**3. Too Many Contexts**
- Each context = ~10 MB
- 100 contexts = 1 GB

**Solution**:
```bash
# Remove unused contexts
curl -X DELETE http://localhost:2428/contexts/:context_id
```

---

## Blob Upload Failures

### Symptom
Blobs fail to upload or retrieve.

### Diagnosis
```bash
# Check blob manager status
curl http://localhost:2428/debug/blobs

# Try manual upload
curl -X POST http://localhost:2428/blobs \
  -H "Content-Type: application/octet-stream" \
  --data-binary @test.bin
```

### Common Causes

**1. Blob Too Large**
- Exceeds size limit
- Network timeout

**Solution**:
```rust
// Check max blob size config
const MAX_BLOB_SIZE: usize = 10 * 1024 * 1024;  // 10 MB

// Chunk large files
```

**2. Storage Full**
- Disk space exhausted
- Blob storage quota exceeded

**Solution**:
```bash
# Check disk space
df -h

# Run garbage collection
curl -X POST http://localhost:2428/admin/gc
```

---

## Hash Divergence

### Symptom
Nodes have same DAG heads but different root hashes.

### Diagnosis
```bash
# Compare root hashes
curl http://localhost:2428/debug/hash/:context_id  # Node A
curl http://<node_b>:2428/debug/hash/:context_id   # Node B
```

### Common Causes

**1. Non-Deterministic CRDT**
- CRDT merge not deterministic
- LWW not using HLC correctly

**Solution**:
```rust
// Ensure deterministic merge
impl Merge for MyType {
    fn merge(&mut self, other: &Self) {
        // Use HLC for tie-breaking
        if other.hlc > self.hlc {
            *self = other.clone();
        }
    }
}
```

**2. Apply Order Different**
- Deltas applied in different orders
- Missing parent enforcement

**Solution**:
- DAG ensures correct order
- Check DAG implementation

**3. Corrupted State**
- Storage corruption
- Invalid delta

**Solution**:
```bash
# Trigger full state sync
curl -X POST http://localhost:2428/sync/:context_id/full
```

---

## High Network Traffic

### Symptom
Excessive bandwidth usage.

### Diagnosis
```bash
# Monitor network
nethogs

# Check sync frequency
curl http://localhost:2428/debug/sync_config
```

### Common Causes

**1. Sync Too Frequent**
- `frequency` too low
- `interval` too low

**Solution**:
```rust
let config = SyncConfig {
    frequency: Duration::from_secs(30),
    interval: Duration::from_secs(15),
    ..Default::default()
};
```

**2. Too Many Concurrent Syncs**
- `max_concurrent` too high

**Solution**:
```rust
let config = SyncConfig {
    max_concurrent: 10,
    ..Default::default()
};
```

**3. Large State Transfers**
- Full snapshots instead of deltas
- `delta_sync_threshold` too low

**Solution**:
```rust
let config = SyncConfig {
    delta_sync_threshold: 256,
    ..Default::default()
};
```

---

## See Also

- [Sync Configuration](sync-configuration.md) - How to configure sync
- [Performance](performance.md) - Performance tuning
- [Architecture](architecture.md) - How it works
- [DAG Troubleshooting](../../dag/readme/troubleshooting.md) - DAG-specific issues

# Calimero Sync - Distributed State Synchronization

> **Actor-free sync orchestration for distributed nodes**

Coordinate state synchronization between Calimero nodes using clean async Rust. No actors, no message passing - just strategies, retry logic, and observability.

---

## Quick Start

```rust
use calimero_sync::{SyncScheduler, SyncConfig, strategies::DagCatchup};

// Create scheduler (no actors!)
let scheduler = SyncScheduler::new(
    node_client,
    context_client,
    network_client,
    SyncConfig::default(),
);

// Create strategy
let strategy = DagCatchup::new(
    network_client,
    context_client,
    Duration::from_secs(10),
);

// Sync a context
let result = scheduler.sync_context(
    &context_id,
    &peer_id,
    &our_identity,
    &delta_store,
    &strategy,
).await?;

match result {
    SyncResult::NoSyncNeeded => println!("Already in sync"),
    SyncResult::DeltaSync => println!("Fetched missing deltas"),
    SyncResult::FullResync => println!("Full state resync"),
}
```

**What you get:**
- ✅ **No actors** - plain async Rust
- ✅ **Retry with backoff** - automatic failure recovery
- ✅ **Observable** - events for every sync operation
- ✅ **Composable** - strategy pattern for different sync approaches

---

## Architecture

```
Application
    ↓
┌──────────────────────────────────────────┐
│ SyncScheduler                            │
│ • Tracks active syncs                    │
│ • Executes strategies                    │
│ • Handles retries                        │
│ • Emits events                           │
└──────────────────────────────────────────┘
    ↓
┌──────────────────────────────────────────┐
│ Sync Strategies                          │
│ • DagCatchup (fetch missing deltas)      │
│ • StateResync (full state rebuild)       │
└──────────────────────────────────────────┘
    ↓
┌──────────────────────────────────────────┐
│ Protocols (calimero-protocols)           │
│ • delta_request                          │
│ • state_delta broadcast                  │
└──────────────────────────────────────────┘
    ↓
┌──────────────────────────────────────────┐
│ Network (libp2p)                         │
└──────────────────────────────────────────┘
```

---

## Sync Strategies

### DagCatchup (Default)

Fetch only missing deltas to fill DAG gaps.

```rust
let strategy = DagCatchup::new(
    network_client,
    context_client,
    timeout,
);

// Fast: only requests missing parents
// Efficient: minimal network transfer
// Ideal for: normal operation, brief disconnects
```

**When used:**
- Normal operation
- Brief network disconnects
- DAG has only a few missing deltas

**Performance:** O(missing deltas) - usually < 100ms

### StateResync (Fallback)

Full state rebuild from scratch.

```rust
let strategy = StateResync::new(...);

// Slower: rebuilds entire state
// Comprehensive: guaranteed consistency
// Ideal for: long disconnects, corruption recovery
```

**When used:**
- Extended offline period
- DAG divergence detected
- State corruption suspected

**Performance:** O(full state) - can take seconds for large states

---

## Configuration

```rust
use calimero_sync::{SyncConfig, RetryConfig};

let config = SyncConfig {
    // Maximum concurrent syncs (default: 10)
    max_concurrent_syncs: 10,
    
    // Retry configuration
    retry_config: RetryConfig {
        max_attempts: 5,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(30),
        multiplier: 2.0, // Exponential backoff
    },
    
    // Optional heartbeat (default: disabled)
    enable_heartbeat: false,
    heartbeat_interval: Duration::from_secs(30),
};

let scheduler = SyncScheduler::new(..., config);
```

---

## Observability

All sync operations emit events:

```rust
use calimero_sync::{SyncEvent, SyncStatus};

// Listen for sync events
match event {
    SyncEvent {
        context_id,
        peer_id,
        status: SyncStatus::Started,
        attempt,
        ..
    } => {
        println!("Sync started (attempt {})", attempt);
    }
    
    SyncEvent {
        status: SyncStatus::Completed(result),
        duration,
        ..
    } => {
        println!("Sync completed in {:?}: {:?}", duration, result);
    }
    
    SyncEvent {
        status: SyncStatus::Failed(error),
        attempt,
        ..
    } => {
        println!("Sync failed (attempt {}): {}", attempt, error);
    }
}
```

**Event fields:**
- `context_id` - Which context is being synced
- `peer_id` - Which peer we're syncing with
- `status` - Started | Completed | Failed
- `attempt` - Retry attempt number
- `duration` - How long it took (for completed)

---

## Retry Logic

Automatic exponential backoff on failures:

```
Attempt 1: 100ms delay
Attempt 2: 200ms delay
Attempt 3: 400ms delay
Attempt 4: 800ms delay
Attempt 5: 1.6s delay
...
Max delay: 30s (configurable)
Max attempts: 5 (configurable)
```

**Gives up after:**
- Max attempts reached
- Permanent error (not retryable)

---

## Common Patterns

### Pattern 1: Sync on Context Join

```rust
// New member joining a context
let result = scheduler.sync_context(
    &context_id,
    &inviter_peer_id,
    &my_identity,
    &delta_store,
    &DagCatchup::new(...),
).await?;

// Now we have all the deltas!
```

### Pattern 2: Periodic Sync Check

```rust
// Heartbeat: detect and fix drift
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        
        for (context_id, peer_id) in get_active_contexts() {
            scheduler.sync_context(&context_id, &peer_id, ...).await.ok();
        }
    }
});
```

### Pattern 3: Recovery from Divergence

```rust
// Detected DAG divergence - force full resync
let result = scheduler.sync_context(
    &context_id,
    &peer_id,
    &my_identity,
    &delta_store,
    &StateResync::new(...), // Full rebuild
).await?;
```

---

## Testing

Sync operations are fully testable without infrastructure:

```rust
#[tokio::test]
async fn test_dag_catchup() {
    let strategy = DagCatchup::new(mock_network, mock_context, timeout);
    let mock_delta_store = MockDeltaStore::new();
    
    let result = strategy.sync(
        &context_id,
        &peer_id,
        &our_identity,
        &mock_delta_store,
    ).await?;
    
    assert_eq!(result, SyncResult::DeltaSync);
    assert_eq!(mock_delta_store.delta_count(), 3);
}
```

---

## API Reference

### SyncScheduler

```rust
// Create scheduler
SyncScheduler::new(
    node_client: NodeClient,
    context_client: ContextClient,
    network_client: NetworkClient,
    config: SyncConfig,
) -> Self

// Sync a context
async fn sync_context(
    context_id: &ContextId,
    peer_id: &libp2p::PeerId,
    our_identity: &PublicKey,
    delta_store: &impl DeltaStore,
    strategy: &impl SyncStrategy,
) -> Result<SyncResult>
```

### SyncStrategy Trait

```rust
#[async_trait(?Send)]
pub trait SyncStrategy {
    async fn sync(
        &self,
        context_id: &ContextId,
        peer_id: &libp2p::PeerId,
        our_identity: &PublicKey,
        delta_store: &dyn DeltaStore,
    ) -> Result<SyncResult>;
}
```

### SyncResult

```rust
pub enum SyncResult {
    NoSyncNeeded,  // Already in sync
    DeltaSync,     // Fetched missing deltas
    FullResync,    // Full state rebuild
}
```

---

## Performance

| Operation | Latency | Notes |
|-----------|---------|-------|
| **DagCatchup (1 delta)** | ~100ms | Network RTT |
| **DagCatchup (10 deltas)** | ~200ms | Batched requests |
| **StateResync (small)** | ~500ms | Full rebuild |
| **StateResync (large)** | ~2-5s | Depends on state size |
| **Retry backoff** | 100ms-30s | Exponential |

---

## Design Principles

1. **No actors** - Plain async Rust, easy to reason about
2. **Observable** - Every operation emits events
3. **Resilient** - Automatic retry with backoff
4. **Composable** - Strategy pattern for extensibility
5. **Testable** - No infrastructure dependencies

---

## FAQ

**Q: How is this different from the old SyncManager?**  
A: No actors! Old SyncManager was 1,088 lines of actor code. New SyncScheduler is 400 lines of clean async.

**Q: When should I use DagCatchup vs StateResync?**  
A: DagCatchup for normal operation (fast). StateResync for recovery (comprehensive).

**Q: What happens if a sync fails?**  
A: Automatic retry with exponential backoff (configurable).

**Q: Can I implement custom sync strategies?**  
A: Yes! Just implement the `SyncStrategy` trait.

**Q: How do I know when a sync completes?**  
A: Listen for `SyncEvent` with `status: SyncStatus::Completed`.

---

## License

See root [LICENSE](../../LICENSE) file.

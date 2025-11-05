# calimero-sync

**Sync orchestration for Calimero nodes - NO ACTORS!**

This crate provides clean, async sync orchestration using stateless protocols from `calimero-protocols`.

## Architecture

```text
SyncScheduler (orchestration)
    ↓
Strategies (dag_catchup, state_resync)
    ↓
Protocols (stateless functions)
    ↓
Network (libp2p streams)
```

## Key Differences from Old Architecture

**Old (SyncManager)**:
- ❌ Actor-based (Actix)
- ❌ Message passing
- ❌ Tight coupling
- ❌ Hard to test
- ❌ 1,088 lines of complexity

**New (SyncScheduler)**:
- ✅ Plain async Rust
- ✅ Event-driven
- ✅ Protocol composition
- ✅ Easy to test
- ✅ ~200 lines of clean code

## Usage

```rust
use calimero_sync::{SyncScheduler, SyncConfig};
use calimero_sync::strategies::DagCatchup;

// Create scheduler (NO actors!)
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
    timeout,
);

// Sync a context (plain async!)
let result = scheduler.sync_context(
    &context_id,
    &peer_id,
    &our_identity,
    &delta_store,
    &strategy,
).await?;

match result {
    SyncResult::NoSyncNeeded => println!("Already in sync"),
    SyncResult::DeltaSync { deltas_applied } => {
        println!("Applied {} deltas", deltas_applied)
    }
    SyncResult::FullResync { root_hash } => {
        println!("Full resync completed: {:?}", root_hash)
    }
}
```

## Components

### SyncScheduler
Main orchestration component that:
- Tracks active syncs
- Executes strategies with retry logic
- Emits sync events for observability
- Manages periodic heartbeat (optional)

### Strategies
- **DagCatchup**: Fetch missing deltas (most common)
- **StateResync**: Full state resync (fallback, stub for now)

### Events
- `SyncEvent` - for observability
- `SyncStatus` - started, completed, failed

### Config
- `SyncConfig` - timeout, retries, heartbeat
- `RetryConfig` - exponential backoff

## Design Principles

1. **Stateless** - All dependencies injected
2. **Composable** - Strategies are interchangeable
3. **Testable** - No infrastructure needed
4. **Event-driven** - Observability built-in
5. **NO ACTORS!** - Plain async Rust

## What's Next

Week 3: New `calimero-node` runtime that uses this sync crate!


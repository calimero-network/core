# Integrating calimero-sync into the Node

## Current Architecture

The node has TWO levels of sync management:

### 1. Node-Level `SyncManager` (network orchestration)
**Location**: `crates/node/src/sync.rs`
**Role**: Network-level sync orchestration
- Manages network streams and encryption
- Handles periodic sync scheduling
- Coordinates with peers over libp2p
- Currently tries delta sync, then falls back to state sync

### 2. Storage-Level `SyncManager` (NEW - from calimero-sync)
**Location**: `crates/sync/src/manager.rs`  
**Role**: Decides which sync strategy to use based on storage state
- Determines: Full / Delta / Live sync
- Uses `SyncState` to track peer sync history
- Makes intelligent decisions based on offline duration

## Integration Points

### Key Decision Point

**Current code** (`crates/node/src/sync.rs:389-405`):
```rust
// Try delta sync first, fall back to state sync on failure
match self
    .initiate_delta_sync_process(&mut context, our_identity, &mut stream)
    .await
{
    Ok(()) => Ok(()),
    Err(e) => {
        warn!("Delta sync failed, falling back to state sync");
        self.initiate_state_sync_process(&mut context, our_identity, &mut stream)
            .await
    }
}
```

**Should become**:
```rust
// Use SyncManager to determine strategy based on peer state
use calimero_sync::SyncManager as StorageSyncManager;

let storage_manager = StorageSyncManager::<RocksDBStorage>::new();
let peer_id = Id::from(their_identity);  // Convert libp2p PeerId to storage Id

match storage_manager.determine_sync_strategy(peer_id)? {
    SyncStrategy::Full => {
        // NEW: Full resync via snapshot
        self.initiate_full_resync_process(&mut context, our_identity, &mut stream)
            .await
    }
    SyncStrategy::Delta => {
        // Existing delta sync
        self.initiate_delta_sync_process(&mut context, our_identity, &mut stream)
            .await
    }
    SyncStrategy::Live => {
        // FUTURE: Real-time action streaming
        // For now, fall back to delta
        self.initiate_delta_sync_process(&mut context, our_identity, &mut stream)
            .await
    }
}
```

## Required Changes

### 1. Add Full Resync Support to Node

Create `crates/node/src/sync/full.rs`:

```rust
use calimero_sync::full::{full_resync, generate_snapshot};
use calimero_sync::Snapshot;

impl SyncManager {
    /// Initiates full resync by requesting snapshot from peer
    pub(super) async fn initiate_full_resync_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(context_id=%context.id, "Initiating full resync");

        // 1. Request snapshot from peer
        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::FullSync {  // NEW payload type
                    application_id: context.application_id,
                },
                next_nonce: thread_rng().gen(),
            },
            None,
        )
        .await?;

        // 2. Receive snapshot
        let snapshot = self.recv_snapshot(stream).await?;

        // 3. Apply snapshot using calimero-sync
        full_resync::<RocksDBStorage>(
            Id::from(their_identity),
            snapshot
        )?;

        Ok(())
    }

    /// Handles incoming full resync request
    pub(super) async fn handle_full_resync_request(
        &self,
        context: &Context,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(context_id=%context.id, "Handling full resync request");

        // 1. Generate snapshot using calimero-sync
        let snapshot = self.generate_snapshot_for_context(context.id)?;

        // 2. Send snapshot to peer
        self.send_snapshot(stream, &snapshot).await?;

        Ok(())
    }

    /// Generates snapshot for a specific context from RocksDB
    fn generate_snapshot_for_context(
        &self,
        context_id: ContextId,
    ) -> eyre::Result<Snapshot> {
        // Use node-level snapshot generation from calimero-sync
        calimero_sync::full::generate_snapshot_for_context(
            &self.node_client.store(),  // Access to RocksDB
            &context_id,
        )
    }
}
```

### 2. Update `InitPayload` Enum

In `crates/node-primitives/src/sync.rs`:

```rust
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub enum InitPayload {
    KeyShare,
    BlobShare { blob_id: BlobId },
    StateSync {
        root_hash: Hash,
        application_id: ApplicationId,
    },
    DeltaSync {
        root_hash: Hash,
        application_id: ApplicationId,
    },
    FullSync {  // NEW
        application_id: ApplicationId,
    },
}
```

### 3. Track Peer Sync State

Add `SyncState` tracking in `crates/node/src/sync.rs`:

```rust
use calimero_sync::{get_sync_state, save_sync_state, SyncState as StorageSyncState};

// In initiate_sync_inner, after successful sync:
async fn initiate_sync_inner(...) -> eyre::Result<()> {
    // ... existing sync code ...

    // After successful sync, update sync state
    let peer_id = Id::from(chosen_peer);
    let mut sync_state = get_sync_state::<RocksDBStorage>(peer_id)?
        .unwrap_or_else(|| StorageSyncState::new(peer_id));

    sync_state.update(context.root_hash);
    save_sync_state::<RocksDBStorage>(&sync_state)?;

    Ok(())
}
```

### 4. Create RocksDB Storage Adaptor

The node needs a storage adaptor for RocksDB that implements `IterableStorage`:

```rust
// crates/node/src/storage.rs (NEW FILE)

use calimero_storage::store::{Key, StorageAdaptor, IterableStorage};
use calimero_store::Store;
use calimero_primitives::context::ContextId;

pub struct RocksDBStorage {
    store: Store,
    context_id: ContextId,
}

impl StorageAdaptor for RocksDBStorage {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        // Read from RocksDB using ContextState key
        // Similar to existing node storage access
        todo!()
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        // Write to RocksDB
        todo!()
    }

    fn storage_remove(key: Key) -> bool {
        // Remove from RocksDB
        todo!()
    }
}

impl IterableStorage for RocksDBStorage {
    fn storage_iter_keys() -> Vec<Key> {
        // Iterate RocksDB keys for this context
        todo!()
    }
}
```

## Decision Flow

```text
┌──────────────────────────────────────┐
│ Node receives sync request from peer │
└───────────────┬──────────────────────┘
                │
    ┌───────────▼────────────┐
    │ Convert PeerId → Id    │
    └───────────┬────────────┘
                │
    ┌───────────▼────────────┐
    │ StorageSyncManager     │
    │ .determine_sync        │
    │  _strategy(peer_id)    │
    └───────────┬────────────┘
                │
        ┌───────┴───────┬──────────────────┐
        │               │                  │
┌───────▼──────┐ ┌─────▼──────┐ ┌────────▼────────┐
│ SyncStrategy │ │ SyncStrategy│ │ SyncStrategy    │
│  ::Full      │ │  ::Delta    │ │  ::Live         │
└───────┬──────┘ └──────┬──────┘ └────────┬────────┘
        │               │                  │
┌───────▼──────────┐ ┌──▼───────────┐ ┌──▼──────────┐
│ initiate_full_   │ │ initiate_    │ │ (future)    │
│  resync_process()│ │  delta_sync  │ │ live_sync   │
└──────────────────┘ │  _process()  │ └─────────────┘
                     └──────────────┘
```

## Benefits

1. **Intelligent Strategy Selection**: Uses sync history to choose optimal strategy
2. **Full Resync Support**: Handles nodes offline > 2 days via snapshots
3. **State Tracking**: Maintains peer sync history for better decisions
4. **Separation of Concerns**:
   - `calimero-storage`: CRDT core
   - `calimero-sync`: Sync strategy and state
   - `calimero-node`: Network orchestration

## Rollout Plan

1. **Phase 1**: Add `FullSync` support to node (backward compatible)
2. **Phase 2**: Integrate `StorageSyncManager` decision logic
3. **Phase 3**: Replace old fallback logic with strategy-based routing
4. **Phase 4**: Add live sync support (future)

## Testing

Test scenarios:
- ✅ Fresh node (never synced) → Full resync
- ✅ Node offline < 2 days → Delta sync
- ✅ Node offline > 2 days → Full resync
- ✅ Active node → Delta sync (or live when implemented)


# CIP-XXXX: Hybrid State Synchronization Protocol

| Field | Value |
|-------|-------|
| CIP | XXXX (To be assigned) |
| Title | Hybrid State Synchronization Protocol |
| Author | Calimero Team |
| Status | Draft |
| Type | Standards Track |
| Category | Core |
| Created | 2026-01-30 |

---

## Abstract

This CIP proposes a hybrid synchronization protocol that combines delta-based (CmRDT) and state-based (CvRDT) approaches to efficiently synchronize Merkle tree state between nodes. The protocol:

1. **Automatically selects** the optimal sync strategy based on divergence characteristics
2. **Maintains node liveness** during sync operations via delta buffering
3. **Ensures cryptographic verification** of synchronized state
4. **Implements hybrid merge dispatch** where built-in CRDTs merge in storage layer (fast, ~100ns) and custom Mergeable types dispatch to WASM (flexible, ~10μs)

## Motivation

The current synchronization implementation has several limitations:

1. **Fresh Node Bootstrap**: New nodes must fetch ALL deltas from genesis, which is inefficient for contexts with long history (thousands of deltas).

2. **Missing Delta Recovery**: When too many deltas are missing (network partition, offline period), delta-based sync becomes impractical.

3. **No Protocol Selection**: There's no mechanism to choose between different sync strategies based on the situation.

4. **Sync Blocking**: The relationship between ongoing sync and incoming deltas is not well-defined, risking state inconsistency.

5. **No State Verification**: Snapshot transfers don't have cryptographic verification against Merkle root hashes.

6. **CRDT Merge Not Used in State Sync**: State-based sync uses Last-Write-Wins (LWW) instead of proper CRDT merge semantics, causing data loss when concurrent updates occur on built-in CRDTs (Counter, Map, etc.).

7. **Custom Merge Logic Inaccessible**: Apps can define custom `Mergeable` implementations in WASM, but state sync cannot invoke them - it always falls back to LWW.

### Use Cases

| Scenario | Current Behavior | Proposed Behavior |
|----------|------------------|-------------------|
| Fresh node joins | Fetch ALL deltas recursively | Snapshot sync with verification |
| 1% divergence | Fetch missing deltas | Hash-based incremental sync |
| 50% divergence | Fetch ALL missing deltas | Snapshot sync (more efficient) |
| Network partition recovery | May timeout/fail | Adaptive protocol selection |
| Malicious snapshot | Blindly accepted | Cryptographic verification |
| Counter conflict (state sync) | LWW - **data loss!** | Sum per-node counts (CRDT merge) |
| Map conflict (state sync) | LWW - **data loss!** | Per-key merge (preserves all keys) |
| Custom type conflict | LWW only | WASM callback for app-defined merge |
| Root state conflict | LWW | WASM merge_root_state callback |

## Protocol Invariants

These invariants MUST hold for any compliant implementation:

### Convergence Invariants

**I1. Operation Completeness**
> If node A applies operation O, and A syncs with B, then B will eventually have O reflected in its state.

**I2. Eventual Consistency**
> Given no new operations, all connected nodes will converge to identical root hashes within O(log N) sync rounds.

**I3. Merge Determinism**
> For any two values V1, V2 and metadata M1, M2: `merge(V1, V2, M1, M2)` always produces the same output.

**I4. Strategy Equivalence**
> All state-based strategies (HashComparison, BloomFilter, SubtreePrefetch, LevelWise) MUST produce identical final state given identical inputs, differing only in network efficiency.

### Safety Invariants

**I5. No Silent Data Loss**
> State-based sync on initialized nodes MUST use CRDT merge. LWW overwrite is ONLY permitted when local value is absent (fresh node bootstrap).

**I6. Liveness Guarantee**
> Deltas received during state-based sync MUST be preserved and applied after sync completes. Implementations MUST NOT drop buffered deltas.

**I7. Verification Before Apply**
> Snapshot data MUST be verified against claimed root hash BEFORE any state modification.

**I8. Causal Consistency**
> A delta D can only be applied after ALL its parent deltas have been applied. The DAG structure enforces this.

### Identity Invariants

**I9. Deterministic Entity IDs**
> Given the same application code and field names, all nodes MUST generate identical entity IDs for the same logical entities. Non-deterministic IDs cause "ghost entities" that prevent proper CRDT merge.

**I10. Metadata Persistence**
> Entity metadata (including `crdt_type`) MUST be persisted alongside entity data. Metadata loss forces LWW fallback and potential data loss.

### Protocol Behavior Invariants

**I11. Protocol Honesty**
> A node MUST NOT advertise a protocol in `SyncCapabilities` unless it can execute the protocol end-to-end (diff discovery AND entity transfer).

**I12. SyncProtocol::None Behavior**
> When `SyncProtocol::None` is selected (root hashes match), responder MUST acknowledge without data transfer. This is distinguishable from negotiation failure.

---

## Specification

### 1. Sync Protocol Types

```rust
pub enum SyncProtocol {
    /// No sync needed - already in sync
    None,
    
    /// Delta-based sync via DAG (existing)
    DeltaSync {
        missing_delta_ids: Vec<[u8; 32]>,
    },
    
    /// Hash-based Merkle tree comparison
    HashComparison {
        root_hash: [u8; 32],
        divergent_subtrees: Vec<Id>,
    },
    
    /// Full state snapshot transfer
    Snapshot {
        compressed: bool,
        verified: bool,
    },
    
    /// Bloom filter quick diff
    BloomFilter {
        filter_size: usize,
        false_positive_rate: f64,
    },
    
    /// Subtree prefetch for deep localized changes
    SubtreePrefetch {
        subtree_roots: Vec<Id>,
    },
    
    /// Level-wise sync for wide shallow trees
    LevelWise {
        max_depth: usize,
    },
}
```

### 2. Protocol Negotiation

#### 2.1 Handshake Message

```rust
pub struct SyncHandshake {
    /// Our current root hash
    pub root_hash: [u8; 32],
    
    /// Whether we have any state
    pub has_state: bool,
    
    /// Number of entities in our tree
    pub entity_count: usize,
    
    /// Maximum tree depth
    pub max_depth: usize,
    
    /// Our DAG heads (for delta sync compatibility)
    pub dag_heads: Vec<[u8; 32]>,
    
    /// Supported protocols (ordered by preference)
    pub supported_protocols: Vec<SyncProtocol>,
}
```

#### 2.2 Negotiation Flow

```
Requester                              Responder
    │                                      │
    │──── SyncHandshake ──────────────────>│
    │                                      │
    │<─── SyncHandshake ───────────────────│
    │                                      │
    │     (Both compute optimal protocol)  │
    │                                      │
    │──── ProtocolSelected { protocol } ──>│
    │                                      │
    │<─── ProtocolAck / ProtocolNack ──────│
    │                                      │
    │     (Begin selected protocol)        │
```

#### 2.3 Protocol Selection Rules

Protocol selection MUST follow these rules in order:

**Decision Table:**

| # | Condition | Selected Protocol | Rationale |
|---|-----------|-------------------|-----------|
| 1 | `local.root_hash == remote.root_hash` | `None` | Already synchronized |
| 2 | `!local.has_state` (fresh node) | `Snapshot` | Full bootstrap required |
| 3 | `local.has_state` AND divergence > 50% | `HashComparison` | Large diff, MUST use CRDT merge |
| 4 | `max_depth > 3` AND divergence < 20% | `SubtreePrefetch` | Deep tree, localized changes |
| 5 | `entity_count > 50` AND divergence < 10% | `BloomFilter` | Large tree, small diff |
| 6 | `max_depth <= 2` AND many children | `LevelWise` | Wide shallow tree |
| 7 | (default) | `HashComparison` | General-purpose fallback |

**Divergence Calculation:**

```
divergence_ratio = |local.entity_count - remote.entity_count| / max(remote.entity_count, 1)
```

**Fallback Rules:**

1. If the preferred protocol is not in `remote.supported_protocols`, implementations MUST fall back to the next applicable row in the decision table.
2. `DeltaSync` MAY be used as a final fallback if no state-based protocol is mutually supported.
3. Implementations MUST NOT select `Snapshot` for initialized nodes (see Invariant I5).

**Compression:**

- `Snapshot` SHOULD use compression when `remote.entity_count > 100`
- Compression algorithm SHOULD be negotiated in handshake extensions

### 3. Sync Hints in Delta Propagation

When a node applies a local delta and propagates it, include **sync hints** to help receivers decide proactively if they need a full sync instead of waiting to discover divergence.

#### 3.1 Enhanced Delta Message

```rust
pub struct DeltaWithHints {
    /// The actual delta
    pub delta: CausalDelta,
    
    /// Sync hints for receivers
    pub hints: SyncHints,
}

pub struct SyncHints {
    /// Current root hash after applying this delta
    pub root_hash: [u8; 32],
    
    /// Total entity count in tree
    pub entity_count: usize,
    
    /// How many deltas since genesis (chain height)
    pub delta_height: u64,
    
    /// Number of deltas in last N minutes (activity indicator)
    pub recent_delta_count: u32,
    
    /// Bloom filter of all delta IDs we have
    /// (compact way to detect missing deltas)
    pub delta_bloom_filter: Option<Vec<u8>>,
    
    /// Estimated "age" - oldest missing ancestor we know about
    pub oldest_pending_parent: Option<[u8; 32]>,
}
```

#### 3.2 Receiver Decision Logic

When a node receives a delta with hints, it can immediately determine its sync strategy:

```rust
impl SyncManager {
    fn on_delta_received(&mut self, msg: DeltaWithHints) -> SyncDecision {
        let hints = &msg.hints;
        
        // 1. Check if we're already in sync
        if self.root_hash == hints.root_hash {
            return SyncDecision::AlreadySynced;
        }
        
        // 2. Check if we have the parent deltas
        let missing_parents: Vec<[u8; 32]> = msg.delta.parents
            .iter()
            .filter(|p| !self.dag_store.has_delta(p))
            .copied()
            .collect();
        
        if !missing_parents.is_empty() {
            // Missing parents - how many?
            let our_height = self.dag_store.height();
            let gap = hints.delta_height.saturating_sub(our_height);
            
            if gap > DELTA_SYNC_THRESHOLD {
                // Too far behind - request snapshot instead of chasing deltas
                return SyncDecision::RequestSnapshot {
                    peer: msg.sender,
                    reason: SyncReason::TooFarBehind { gap },
                };
            }
            
            // Small gap - request missing parent deltas first
            return SyncDecision::RequestMissingDeltas {
                delta_ids: missing_parents,
            };
        }
        
        // 3. Use bloom filter to estimate missing deltas
        if let Some(ref bloom) = hints.delta_bloom_filter {
            let missing_estimate = self.estimate_missing_from_bloom(bloom);
            
            if missing_estimate > DELTA_SYNC_THRESHOLD {
                return SyncDecision::RequestSnapshot {
                    peer: msg.sender,
                    reason: SyncReason::TooManyMissing { estimate: missing_estimate },
                };
            }
        }
        
        // 4. Entity count divergence check
        let our_count = self.entity_count();
        let count_diff = (our_count as i64 - hints.entity_count as i64).abs();
        let divergence = count_diff as f32 / hints.entity_count.max(1) as f32;
        
        if divergence > 0.5 {
            return SyncDecision::RequestHashSync {
                peer: msg.sender,
                reason: SyncReason::SignificantDivergence { ratio: divergence },
            };
        }
        
        // 5. All parents present - safe to apply
        SyncDecision::ApplyDelta(msg.delta)
    }
}

pub enum SyncDecision {
    AlreadySynced,
    ApplyDelta(CausalDelta),
    RequestMissingDeltas { delta_ids: Vec<[u8; 32]> },
    RequestHashSync { peer: PeerId, reason: SyncReason },
    RequestSnapshot { peer: PeerId, reason: SyncReason },
}

pub enum SyncReason {
    TooFarBehind { gap: u64 },
    TooManyMissing { estimate: usize },
    SignificantDivergence { ratio: f32 },
    FreshNode,
}
```

#### 3.3 Lightweight Hints (Minimal Overhead)

For nodes concerned about bandwidth, a minimal hint set:

```rust
pub struct LightweightHints {
    /// Just the root hash - receivers can compare
    pub root_hash: [u8; 32],
    
    /// Delta height - single number to detect gaps
    pub delta_height: u64,
}
```

**Overhead:** Only 40 bytes per delta propagation.

#### 3.4 Proactive Sync Triggers

With hints, sync can be triggered **proactively** instead of reactively:

| Trigger | Without Hints | With Hints |
|---------|---------------|------------|
| Fresh node joins | Waits for first delta, then discovers gap | Immediately sees `delta_height` gap |
| Network partition heals | Tries delta sync, times out, then retries | Sees `root_hash` mismatch + `delta_height` gap |
| Slow node catches up | Recursively fetches deltas one by one | Sees gap > threshold, requests snapshot |
| Malicious delta | Applies, then discovers state mismatch | Verifies `root_hash` matches expected |

#### 3.5 Gossip Protocol Enhancement

Delta gossip can include hints at different verbosity levels:

```rust
pub enum GossipMode {
    /// Just the delta (current behavior)
    DeltaOnly,
    
    /// Delta + lightweight hints (40 bytes extra)
    WithLightHints,
    
    /// Delta + full hints (for nodes returning from offline)
    WithFullHints,
    
    /// Periodic announcement of state (no delta, just hints)
    StateAnnouncement,
}
```

**State Announcements** allow nodes to periodically broadcast their state summary, enabling peers to detect divergence even without active delta propagation:

```rust
impl SyncManager {
    /// Periodic state announcement (e.g., every 30 seconds)
    fn announce_state(&self) {
        let announcement = SyncHints {
            root_hash: self.root_hash,
            entity_count: self.entity_count(),
            delta_height: self.dag_store.height(),
            recent_delta_count: self.recent_delta_count(),
            delta_bloom_filter: Some(self.dag_store.bloom_filter()),
            oldest_pending_parent: None,
        };
        
        self.network.gossip(GossipMessage::StateAnnouncement(announcement));
    }
}
```

### 4. Sync State Machine

```
SYNC STATE MACHINE
==================

    ┌──────────────────────────────────────────────────────────────────┐
    │                           IDLE                                    │
    │  Waiting for sync trigger (timer, hint, or manual request)       │
    └──────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Trigger: divergence detected,
                                    │          periodic timer, or
                                    │          fresh node join
                                    ▼
    ┌──────────────────────────────────────────────────────────────────┐
    │                        NEGOTIATING                                │
    │  Exchange SyncHandshake with peer:                               │
    │  - Our root hash, entity count, DAG heads                        │
    │  - Peer's root hash, entity count, DAG heads                     │
    │  - Agree on protocol based on divergence                         │
    └──────────────────────────────────────────────────────────────────┘
                                    │
                    ┌───────────────┼───────────────┐
                    │               │               │
                    ▼               ▼               ▼
    ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐
    │   DELTA SYNC     │ │   HASH SYNC      │ │   STATE SYNC     │
    │                  │ │                  │ │   (Snapshot)     │
    │ When: Few deltas │ │ When: Unknown    │ │ When: Fresh node │
    │ missing, DAG     │ │ divergence,      │ │ or massive       │
    │ heads known      │ │ 1-50% different  │ │ divergence       │
    │                  │ │                  │ │                  │
    │ How: Request     │ │ How: Compare     │ │ How: Transfer    │
    │ specific deltas  │ │ tree hashes,     │ │ entire state     │
    │ by ID            │ │ fetch differing  │ │ (compressed,     │
    │                  │ │ leaves only      │ │ paginated)       │
    │                  │ │                  │ │                  │
    │ Cost: O(missing) │ │ Cost: O(log n)   │ │ Cost: O(n)       │
    └────────┬─────────┘ └────────┬─────────┘ └────────┬─────────┘
             │                    │                    │
             └────────────────────┼────────────────────┘
                                  │
                                  ▼
    ┌──────────────────────────────────────────────────────────────────┐
    │                        VERIFYING                                  │
    │  - Snapshot: computed root MUST equal claimed root               │
    │  - Post-merge: local root MAY differ (see Section 7)             │
    └──────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
    ┌──────────────────────────────────────────────────────────────────┐
    │                        APPLYING                                   │
    │  - Delta sync: replay operations via WASM                        │
    │  - Hash sync: CRDT merge differing entities                      │
    │  - State sync: apply snapshot + create checkpoint delta          │
    └──────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
    ┌──────────────────────────────────────────────────────────────────┐
    │                           IDLE                                    │
    │  Sync complete. Root hashes now match (eventually consistent).   │
    └──────────────────────────────────────────────────────────────────┘
```

**Protocol Selection Decision Tree:**

```
Is local state empty?
    │
    ├─ YES ──► STATE SYNC (Snapshot)
    │          Fastest way to bootstrap
    │
    └─ NO ──► Do we know which deltas are missing?
                  │
                  ├─ YES, and < 50 missing ──► DELTA SYNC
                  │                            Fetch by ID
                  │
                  └─ NO or too many ──► HASH SYNC
                                        Compare trees to find differences
```

### 5. Delta Handling During Sync

#### 5.1 Delta Buffer

During state-based sync (snapshot, hash comparison), incoming deltas MUST be buffered:

```rust
pub struct SyncContext {
    /// Current sync state
    state: SyncState,
    
    /// Deltas received during sync (buffered)
    buffered_deltas: Vec<CausalDelta>,
    
    /// Snapshot of root hash when sync started
    sync_start_root_hash: [u8; 32],
    
    /// HLC timestamp when sync started (for filtering buffered deltas)
    sync_start_hlc: HybridTimestamp,
    
    /// Root hash received from peer
    peer_root_hash: [u8; 32],
    
    /// DAG store reference
    dag_store: DagStore,
}

impl SyncContext {
    /// Handle incoming delta during sync
    pub fn on_delta_received(&mut self, delta: CausalDelta) {
        match self.state {
            SyncState::Idle => {
                // Normal operation - apply immediately
                self.dag_store.add_delta(delta);
            }
            SyncState::DeltaSyncing => {
                // Delta sync in progress - add to DAG (may go pending)
                self.dag_store.add_delta(delta);
            }
            SyncState::StateSyncing | SyncState::HashSyncing => {
                // State-based sync - BUFFER for later
                self.buffered_deltas.push(delta);
            }
            SyncState::Verifying | SyncState::Applying => {
                // Buffer until sync completes
                self.buffered_deltas.push(delta);
            }
        }
    }
}
```

#### 5.2 Post-Sync Delta Replay

After state-based sync completes, buffered deltas MUST be replayed via **DAG insertion** (not HLC sorting).

> ⚠️ **CRITICAL**: HLC ordering does NOT guarantee causal ordering. A delta's parent may have a higher HLC due to clock skew. DAG insertion ensures parents are applied before children regardless of timestamp.

```rust
impl SyncContext {
    pub async fn finalize_sync(&mut self) -> Result<()> {
        // 1. Verify received state
        self.verify_snapshot()?;
        
        // 2. Apply received state (CRDT merge for initialized nodes)
        self.apply_snapshot()?;
        
        // 3. Replay buffered deltas via DAG insertion (NOT HLC sort!)
        // The DAG enforces causal ordering: parents applied before children
        for delta in self.buffered_deltas.drain(..) {
            // Add to DAG - may queue if parents still missing
            self.dag_store.add_delta(delta).await;
        }
        
        // 4. Apply all ready deltas in causal order
        // DAG tracks parent dependencies and applies when ready
        self.dag_store.apply_ready_deltas().await?;
        
        // 5. Transition to idle
        self.state = SyncState::Idle;
        
        Ok(())
    }
}
```

**Why DAG, not HLC?**

| Approach | Ordering | Clock Skew Safe? | Causal? |
|----------|----------|------------------|---------|
| HLC Sort | Timestamp | ❌ No | ❌ No |
| DAG Insert | Parent hashes | Yes | Yes |

The DAG tracks parent-child relationships via hashes, not timestamps, ensuring correct causal ordering even with clock skew.

### 6. Snapshot Usage Constraints

Snapshot sync has different semantics depending on the receiver's state:

#### 6.1 Fresh Node Bootstrap (Snapshot as Initialization)

| Condition | `local.has_state == false` |
|-----------|---------------------------|
| Behavior | Apply snapshot directly (no CRDT merge) |
| Post-condition | `local_root == snapshot_root` |
| Use case | New node joining network |

```rust
// Fresh node: direct application
if !local.has_state {
    apply_snapshot_direct(snapshot);  // No merge needed
    assert_eq!(local_root, snapshot.root_hash);
}
```

#### 6.2 Initialized Node Sync (Snapshot as CRDT State)

| Condition | `local.has_state == true` |
|-----------|--------------------------|
| Behavior | CRDT merge each entity |
| Post-condition | `local_root` is merged state (may differ from `snapshot_root`) |
| Use case | Partition healing, large divergence recovery |

```rust
// Initialized node: MUST merge
if local.has_state {
    for entity in snapshot.entities {
        crdt_merge(local_entity, entity);  // Preserves both sides' updates
    }
    // local_root may differ from snapshot.root_hash - that's expected
}
```

#### 6.3 Overwrite Protection (CRITICAL)

> ⚠️ **INVARIANT I5**: An initialized node MUST NOT blindly overwrite state with a snapshot.

**Violation consequences:**
- Data loss (local updates discarded)
- Convergence failure (nodes diverge permanently)
- CRDT invariants broken

```rust
// ❌ INCORRECT: Overwrites local state
fn apply_snapshot_wrong(snapshot: Snapshot) {
    clear_local_state();
    for entity in snapshot.entities {
        write(entity);  // Loses local concurrent updates!
    }
}

// CORRECT: Merges with local state
fn apply_snapshot_correct(snapshot: Snapshot) {
    for entity in snapshot.entities {
        let local = read_local(entity.id);
        let merged = crdt_merge(local, entity);  // Preserves both
        write(merged);
    }
}
```

### 7. Root Hash Semantics

Root hash expectations vary by protocol and scenario:

| Protocol | Scenario | Post-Apply Expectation |
|----------|----------|------------------------|
| DeltaSync | Sequential (no concurrent) | `computed == expected` MUST match |
| DeltaSync | Concurrent (merge) | `computed ≠ expected` - new merged state |
| HashComparison | Normal | `computed == peer_root` SHOULD match |
| HashComparison | Concurrent updates | May differ (apply again) |
| Snapshot | Fresh node | `computed == snapshot_root` MUST match |
| Snapshot | Initialized node (merge) | `computed` is merged state (may differ) |

**When is root hash a HARD invariant?**
- Snapshot integrity verification (before apply)
- Merkle proof verification
- Fresh node bootstrap completion

**When is root hash EMERGENT?**
- Post-CRDT-merge state
- Post-bidirectional-sync state
- After concurrent operations

### 8. Cryptographic Verification

#### 8.1 Snapshot Verification

```rust
impl Snapshot {
    /// Verify all entity hashes match their index entries
    pub fn verify(&self) -> Result<(), VerificationError> {
        for (id, data) in &self.entries {
            // Compute hash of entity data
            let computed_hash = sha256(data);
            
            // Find corresponding index entry
            let index_entry = self.indexes.iter()
                .find(|idx| idx.id() == *id)
                .ok_or(VerificationError::MissingIndex(*id))?;
            
            // Verify hash matches
            if computed_hash != index_entry.own_hash() {
                return Err(VerificationError::HashMismatch {
                    id: *id,
                    expected: index_entry.own_hash(),
                    computed: computed_hash,
                });
            }
        }
        
        // Verify root hash
        let computed_root = self.compute_root_hash();
        if computed_root != self.root_hash {
            return Err(VerificationError::RootHashMismatch {
                expected: self.root_hash,
                computed: computed_root,
            });
        }
        
        Ok(())
    }
}
```

#### 8.2 Incremental Verification

For hash-based sync, verify each entity as received:

```rust
fn verify_entity(
    id: Id,
    data: &[u8],
    comparison: &ComparisonData,
) -> Result<(), VerificationError> {
    let computed_own_hash = sha256(data);
    
    if computed_own_hash != comparison.own_hash {
        return Err(VerificationError::HashMismatch {
            id,
            expected: comparison.own_hash,
            computed: computed_own_hash,
        });
    }
    
    Ok(())
}
```

### 9. Bidirectional Sync

All protocols MUST be bidirectional to ensure convergence:

```rust
pub trait BidirectionalSync {
    /// Perform sync, returning actions for both sides
    fn sync(
        &self,
        channel: &mut NetworkChannel,
    ) -> Result<SyncResult>;
}

pub struct SyncResult {
    /// Actions to apply locally
    pub local_actions: Vec<Action>,
    
    /// Actions to send to peer for application
    pub remote_actions: Vec<Action>,
    
    /// Network statistics
    pub stats: NetworkStats,
}
```

### 10. Network Messages

```rust
pub enum SyncMessage {
    // Handshake
    Handshake(SyncHandshake),
    ProtocolSelected { protocol: SyncProtocol },
    ProtocolAck,
    ProtocolNack { reason: String },
    
    // Hash-based
    RequestEntities { ids: Vec<Id> },
    EntitiesResponse { entities: Vec<(Id, Vec<u8>, ComparisonData)> },
    
    // Snapshot
    RequestSnapshot { compressed: bool },
    SnapshotResponse { snapshot: Snapshot },
    
    // Bloom filter
    BloomFilter { filter: Vec<u8>, root_hash: [u8; 32] },
    BloomDiffResponse { missing: Vec<(Id, Vec<u8>, ComparisonData)> },
    
    // Bidirectional
    ActionsForPeer { actions: Vec<Action> },
    ActionsApplied { count: usize },
    
    // Verification
    VerificationFailed { reason: String },
    
    // Sync Hints (proactive sync triggers)
    DeltaWithHints { delta: CausalDelta, hints: SyncHints },
    StateAnnouncement { hints: SyncHints },
    RequestSyncMode { reason: SyncReason },
}
```

## Rationale

### Why Hybrid Approach?

1. **Delta sync (CmRDT)** is optimal for:
   - Real-time updates (low latency)
   - Small, incremental changes
   - Maintaining causal history

2. **State sync (CvRDT)** is optimal for:
   - Fresh node bootstrap
   - Large divergence recovery
   - Network partition healing

3. **Combining both** provides:
   - Best performance across all scenarios
   - Graceful degradation
   - Automatic recovery

### Why Negotiation?

Without negotiation, nodes might:
- Use incompatible protocols
- Choose suboptimal strategies
- Fail to sync due to capability mismatch

The handshake ensures both nodes agree on the best approach.

### Why Buffer Deltas?

During state-based sync:
- Applying deltas to partial state causes inconsistency
- Ignoring deltas loses data
- Buffering + replay ensures nothing is lost

### Why Bidirectional?

One-directional sync can't achieve root hash convergence when both nodes have unique data. Bidirectional ensures both nodes end up with identical state.

### Why Sync Hints in Delta Propagation?

Without hints, sync is **reactive**:
1. Node receives delta
2. Discovers missing parents
3. Requests parents recursively
4. Eventually times out or succeeds
5. Only then considers alternative sync

With hints, sync is **proactive**:
1. Node receives delta + hints
2. **Immediately** sees gap (delta_height, root_hash mismatch)
3. Makes informed decision: delta sync vs snapshot
4. No wasted round trips chasing deltas

**Key benefits:**
- **Faster recovery**: Fresh nodes don't waste time trying delta sync
- **Less bandwidth**: Avoid fetching 1000s of deltas only to give up
- **Better UX**: Users see "syncing snapshot" instead of hanging
- **Bloom filter efficiency**: O(1) membership test for delta existence

**Overhead is minimal:**
- Lightweight hints: 40 bytes (root_hash + delta_height)
- Full hints: ~200 bytes (with bloom filter)
- Compared to delta payload: Often 100+ bytes

## Backwards Compatibility

This CIP is backwards compatible:

1. **Existing delta sync** remains the default for nodes that don't support new protocols
2. **Handshake** allows capability discovery
3. **Fallback** to delta sync if negotiation fails

## Security Considerations

### 1. Malicious Snapshots

**Risk**: Peer sends tampered snapshot data.
**Mitigation**: Full cryptographic verification before applying.

### 2. Replay Attacks

**Risk**: Peer replays old deltas during sync.
**Mitigation**: HLC timestamps prevent accepting stale data.

### 3. Resource Exhaustion

**Risk**: Peer sends massive snapshot to exhaust memory.
**Mitigation**: Size limits, streaming, and compression.

### 4. Split-Brain

**Risk**: Network partition causes divergent states.
**Mitigation**: Deterministic conflict resolution (LWW, configurable per-entity).

## Acceptance Criteria

### Sync Success vs Convergence

**Sync Session Success** - A single sync exchange between two peers is successful when:
1. All requested entities have been transferred (no protocol errors)
2. All received entities have been applied via CRDT merge
3. Buffered deltas (if any) have been replayed via DAG

**Convergence** - All peers have identical state. May require multiple sync rounds.

> Note: A successful sync does NOT guarantee immediate root hash equality (concurrent operations may occur during sync).

### Black-Box Compliance Tests

| # | Scenario | Observable Behavior | Pass Criteria |
|---|----------|---------------------|---------------|
| **A1** | Fresh node joins | Node bootstraps from peer | `node.root_hash == peer.root_hash` after sync |
| **A2** | Concurrent writes | Two nodes write simultaneously | Both nodes converge to same `root_hash` |
| **A3** | Partition heals | Two partitions reconnect | All nodes converge to same state |
| **A4** | Delta during sync | Delta arrives while snapshot syncing | Delta visible in final state (not lost) |
| **A5** | Counter conflict | Both nodes increment counter | `final_count == node1_increments + node2_increments` |
| **A6** | Map conflict | Both nodes add different keys | All keys present in both nodes |
| **A7** | Custom type merge | Both nodes modify custom type | WASM merge callback invoked, both see merged result |
| **A8** | Malicious snapshot | Peer sends tampered snapshot | Verification fails, sync aborts, no state change |
| **A9** | Large divergence (50%) | Nodes have 50% different entities | Sync completes, states converge |
| **A10** | Identity determinism | Same code on two nodes | Same entity IDs generated |

### Implementation Checkpoints (Definition of Done)

An implementation is considered complete when it satisfies all of the following checkpoints:

#### Core Protocol Checkpoints

| Checkpoint | Requirement |
|------------|-------------|
| CP-1 | `SyncHandshake` messages exchanged and parsed correctly |
| CP-2 | Protocol negotiation selects strategy per decision table (§2.3) |
| CP-3 | `DeltaSync` transfers deltas by ID with parent verification |
| CP-4 | `HashComparison` walks Merkle tree and transfers differing entities |
| CP-5 | `Snapshot` transfers full state with cryptographic verification |
| CP-6 | `BloomFilter` identifies missing entities with configurable FP rate |
| CP-7 | All state-based strategies include `crdt_type` metadata in transfer |

#### CRDT Merge Checkpoints

| Checkpoint | Requirement |
|------------|-------------|
| CP-8 | `Counter` merge sums per-node contribution vectors |
| CP-9 | `UnorderedMap` merge preserves all keys (per-key LWW or recursive) |
| CP-10 | `UnorderedSet` merge is add-wins union |
| CP-11 | `LwwRegister` merge uses HLC timestamp comparison |
| CP-12 | `Vector` merge is element-wise |
| CP-13 | `Rga` merge preserves all insertions (tombstone-based) |
| CP-14 | Custom types dispatch to WASM `merge()` callback |
| CP-15 | Root state conflicts invoke WASM `merge_root_state()` |

#### Safety Checkpoints

| Checkpoint | Requirement |
|------------|-------------|
| CP-16 | Snapshot on initialized node uses CRDT merge (Invariant I5) |
| CP-17 | Deltas received during state sync are buffered (Invariant I6) |
| CP-18 | Buffered deltas replayed via DAG insertion (causal order) |
| CP-19 | Entity metadata (`crdt_type`) persisted with entity data (Invariant I10) |
| CP-20 | Snapshot data verified before any state modification (Invariant I7) |

#### Identity Checkpoints

| Checkpoint | Requirement |
|------------|-------------|
| CP-21 | Entity IDs are deterministic given same code and field names (Invariant I9) |
| CP-22 | Collection IDs derived from parent ID + field name hash |
| CP-23 | No random ID generation for persistent state entities |

#### Verification Checkpoints

| Checkpoint | Requirement |
|------------|-------------|
| CP-24 | Snapshot root hash verified against claimed value |
| CP-25 | Entity hashes verified during tree sync |
| CP-26 | Tampered data rejected with clear error, no state modification |

## Compliance Test Plan

Compliant implementations MUST pass the following black-box test scenarios.

### Protocol Negotiation Tests

| ID | Scenario | Setup | Action | Expected Result |
|----|----------|-------|--------|-----------------|
| N1 | Full capability match | Both nodes support all protocols | Exchange handshakes | Optimal protocol selected per decision table |
| N2 | Mixed capabilities | Node A supports Snapshot, Node B does not | Fresh node A syncs with B | Falls back to DeltaSync or HashComparison |
| N3 | Version mismatch | Nodes have different protocol versions | Handshake exchange | Graceful fallback or clear rejection |
| N4 | Root hash match | Both nodes have identical `root_hash` | Handshake exchange | `SyncProtocol::None` selected, no data transfer |

### Delta Buffering Tests

| ID | Scenario | Setup | Action | Expected Result |
|----|----------|-------|--------|-----------------|
| B1 | Buffer during snapshot | Node syncing via snapshot | Incoming delta arrives | Delta buffered, replayed after sync |
| B2 | Buffer ordering | Multiple deltas arrive during sync | Sync completes | Deltas applied in causal order (via DAG) |
| B3 | Buffer overflow | Very large number of deltas arrive | Sync completes | All deltas preserved (MUST NOT drop) |

### CRDT Merge Tests

| ID | Scenario | Setup | Action | Expected Result |
|----|----------|-------|--------|-----------------|
| M1 | Counter merge | Node A: +5, Node B: +3 | Sync | Final count = 8 |
| M2 | Map disjoint keys | Node A: {a:1}, Node B: {b:2} | Sync | Both nodes have {a:1, b:2} |
| M3 | Map same key | Node A: {k:1}, Node B: {k:2} (later HLC) | Sync | Both nodes have {k:2} |
| M4 | Set union | Node A: {1,2}, Node B: {2,3} | Sync | Both nodes have {1,2,3} |
| M5 | Custom type | Both nodes modify `MyGameState` | Sync | WASM `merge()` callback invoked |
| M6 | Root state merge | Both nodes modify root | Sync | WASM `merge_root_state()` callback invoked |
| M7 | Unknown type fallback | Entity has no `crdt_type` metadata | Sync | LWW applied, no crash |

### End-to-End Convergence Tests

| ID | Scenario | Setup | Action | Expected Result |
|----|----------|-------|--------|-----------------|
| E1 | Two-node concurrent writes | A and B write simultaneously | Sync both directions | `A.root_hash == B.root_hash` |
| E2 | Three-node convergence | A↔B, B↔C, A↔C with concurrent writes | Multiple sync rounds | All three have identical state |
| E3 | Fresh node joins | C has no state, A and B have state | C syncs with A | `C.root_hash == A.root_hash` |
| E4 | Partition heals | Partition [A,B] and [C,D] evolve independently | Reconnect, sync | All four nodes converge |
| E5 | Large state gap | B is 1000 deltas behind A | B syncs with A | B catches up, states match |

### Security Tests

| ID | Scenario | Setup | Action | Expected Result |
|----|----------|-------|--------|-----------------|
| S1 | Tampered snapshot | Malicious peer sends modified entity | Receiver verifies | Verification fails, sync aborts |
| S2 | Wrong root hash | Claimed root ≠ computed root | Receiver verifies | Verification fails, sync aborts |
| S3 | Snapshot on initialized | Initialized node receives snapshot | Apply | CRDT merge used, NOT overwrite |

---

## Appendix A: Hybrid Merge Architecture

### Overview

The merge architecture has two categories of types:

1. **Built-in CRDTs**: Merge logic is deterministic and implemented in the storage layer
2. **Custom Mergeable Types**: Merge logic is defined in WASM by the application

```
┌─────────────────────────────────────────────────────────────────────┐
│                         State Sync                                  │
│                                                                     │
│  On conflict, check metadata.crdt_type:                             │
└─────────────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┴───────────────┐
              │                               │
              ▼                               ▼
┌─────────────────────────────┐   ┌───────────────────────────────────┐
│   Built-in CRDTs            │   │   Custom Mergeable Types          │
│                             │   │                                   │
│   CrdtType::Counter         │   │   CrdtType::Custom {              │
│   CrdtType::UnorderedMap    │   │       type_name: "MyGameState",   │
│   CrdtType::Vector          │   │   }                               │
│   CrdtType::Rga             │   │                                   │
│   CrdtType::UnorderedSet    │   │                                   │
│   CrdtType::LwwRegister     │   │   ┌───────────────────────────┐   │
│                             │   │   │      WASM Module          │   │
│   Merge in Storage Layer    │   │   │                           │   │
│   No WASM needed            │   │   │  impl Mergeable for       │   │
│   ~100ns per merge          │   │   │  MyGameState { ... }      │   │
│                             │   │   └───────────────────────────┘   │
│                             │   │                                   │
│                             │   │   ⚠️ Requires WASM callback      │
│                             │   │   ⚠️ ~10μs per merge             │
└─────────────────────────────┘   └───────────────────────────────────┘
```

### CrdtType Enum

```rust
/// CRDT type for merge dispatch
/// 
/// **All types in state MUST be mergeable!** Non-CRDT types break convergence.
/// Use `LwwRegister<T>` to wrap non-CRDT scalars (String, u64, etc.)
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub enum CrdtType {
    // ══════════════════════════════════════════════════════════════
    // BUILT-IN TYPES: Storage layer merges directly (no WASM needed)
    // ══════════════════════════════════════════════════════════════
    
    /// G-Counter / PN-Counter: Sum per-node counts
    Counter,
    
    /// Last-Write-Wins Register: Higher timestamp wins
    /// Use this to wrap non-CRDT scalars: LwwRegister<String>, LwwRegister<u64>
    LwwRegister,
    
    /// Replicated Growable Array: Tombstone-based text CRDT
    Rga,
    
    /// Unordered Map: Per-key LWW or recursive merge
    UnorderedMap,
    
    /// Unordered Set: Add-wins union
    UnorderedSet,
    
    /// Vector: Element-wise merge
    Vector,
    
    // ══════════════════════════════════════════════════════════════
    // CUSTOM TYPES: Requires WASM callback for merge
    // ══════════════════════════════════════════════════════════════
    
    /// App-defined type with custom merge logic (MUST implement Mergeable)
    Custom {
        /// Type name for WASM dispatch (e.g., "MyGameState")
        type_name: String,
    },
}
```

### Updated Metadata Structure

```rust
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub struct Metadata {
    pub created_at: u64,
    pub updated_at: UpdatedAt,
    pub storage_type: StorageType,
    
    /// CRDT type for merge dispatch
    /// - Built-in types: Merged in storage layer
    /// - Custom types: May require WASM callback
    pub crdt_type: Option<CrdtType>,
    
    /// DEPRECATED: Use crdt_type instead
    /// Kept for backwards compatibility during migration
    #[deprecated(since = "0.5.0", note = "Use crdt_type for merge dispatch")]
    pub resolution: ResolutionStrategy,
}
```

### Merge Decision Table

| Type | Where Merged | WASM? | Performance | Example |
|------|--------------|-------|-------------|---------|
| Counter | Storage | ❌ No | ~100ns | `scores: Counter` |
| UnorderedMap | Storage | ❌ No | ~100ns | `items: UnorderedMap<K,V>` |
| Vector | Storage | ❌ No | ~100ns | `log: Vector<Event>` |
| Rga | Storage | ❌ No | ~100ns | `text: RGA` |
| UnorderedSet | Storage | ❌ No | ~100ns | `tags: UnorderedSet<String>` |
| LwwRegister | Storage | ❌ No | ~100ns | `name: LwwRegister<String>` |
| Custom | WASM | Yes | ~10μs | `game: MyGameState` |
| Root State | WASM | Yes | ~10μs | `#[app::state] MyApp` |
| Unknown (None) | Storage (LWW) | ❌ No | ~100ns | Legacy data only |

> ⚠️ **All state types MUST be mergeable!** Non-CRDT scalars must be wrapped:
> - `name: String` → `name: LwwRegister<String>`
> - `count: u64` → `count: LwwRegister<u64>` or `count: Counter`

### WASM Merge Callback Interface

```rust
/// Trait for WASM merge callback - implemented by runtime
pub trait WasmMergeCallback: Send + Sync {
    /// Merge custom type via WASM
    ///
    /// # Arguments
    /// * `local` - Local entity data (Borsh-serialized)
    /// * `remote` - Remote entity data (Borsh-serialized)
    /// * `type_name` - Type name for dispatch (e.g., "MyGameState")
    ///
    /// # Returns
    /// Merged data (Borsh-serialized)
    fn merge(
        &self,
        local: &[u8],
        remote: &[u8],
        type_name: &str,
    ) -> Result<Vec<u8>, MergeError>;
    
    /// Merge root state (always custom)
    fn merge_root_state(
        &self,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError>;
}

/// Error types for merge operations
#[derive(Debug, Clone)]
pub enum MergeError {
    /// Built-in CRDT merge failed
    CrdtMergeError(String),
    
    /// WASM merge callback not provided for custom type
    WasmCallbackRequired { type_name: String },
    
    /// WASM merge function returned error
    WasmMergeError(String),
    
    /// Serialization/deserialization error
    SerializationError(String),
    
    /// Type mismatch during merge
    TypeMismatch { expected: String, found: String },
}
```

### Hybrid Merge Implementation

```rust
impl<S: StorageAdaptor> Interface<S> {
    /// Merge entity with hybrid dispatch
    pub fn merge_entity(
        local_data: &[u8],
        remote_data: &[u8],
        metadata: &Metadata,
        wasm_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<Vec<u8>, MergeError> {
        match &metadata.crdt_type {
            // ════════════════════════════════════════════════════════
            // BUILT-IN CRDTs: Merge directly in storage layer
            // ════════════════════════════════════════════════════════
            
            Some(CrdtType::Counter) => {
                let mut local: Counter = borsh::from_slice(local_data)
                    .map_err(|e| MergeError::SerializationError(e.to_string()))?;
                let remote: Counter = borsh::from_slice(remote_data)
                    .map_err(|e| MergeError::SerializationError(e.to_string()))?;
                
                local.merge(&remote)
                    .map_err(|e| MergeError::CrdtMergeError(e.to_string()))?;
                
                borsh::to_vec(&local)
                    .map_err(|e| MergeError::SerializationError(e.to_string()))
            }
            
            Some(CrdtType::UnorderedMap) => {
                // Per-key merge with recursive CRDT support
                merge_unordered_map(local_data, remote_data, wasm_callback)
            }
            
            Some(CrdtType::Vector) => {
                merge_vector(local_data, remote_data, wasm_callback)
            }
            
            Some(CrdtType::Rga) => {
                let mut local: ReplicatedGrowableArray = borsh::from_slice(local_data)?;
                let remote: ReplicatedGrowableArray = borsh::from_slice(remote_data)?;
                local.merge(&remote)?;
                Ok(borsh::to_vec(&local)?)
            }
            
            Some(CrdtType::UnorderedSet) => {
                let mut local: UnorderedSet<_> = borsh::from_slice(local_data)?;
                let remote: UnorderedSet<_> = borsh::from_slice(remote_data)?;
                local.merge(&remote)?;
                Ok(borsh::to_vec(&local)?)
            }
            
            Some(CrdtType::LwwRegister) => {
                let mut local: LwwRegister<_> = borsh::from_slice(local_data)?;
                let remote: LwwRegister<_> = borsh::from_slice(remote_data)?;
                local.merge(&remote)?;
                Ok(borsh::to_vec(&local)?)
            }
            
            // ════════════════════════════════════════════════════════
            // CUSTOM TYPES: Dispatch to WASM
            // ════════════════════════════════════════════════════════
            // ONLY for user-defined #[app::state] types.
            // NOT for built-in wrappers like UserStorage/FrozenStorage
            // (those use their underlying collection's CrdtType).
            // All custom types MUST implement Mergeable in WASM.
            
            Some(CrdtType::Custom { type_name }) => {
                // App-defined type - MUST call WASM for merge
                let callback = wasm_callback.ok_or_else(|| {
                    MergeError::WasmCallbackRequired {
                        type_name: type_name.clone(),
                    }
                })?;
                
                callback.merge(local_data, remote_data, type_name)
            }
            
            // ════════════════════════════════════════════════════════
            // FALLBACK: No type info - use LWW
            // ════════════════════════════════════════════════════════
            
            None => {
                // Legacy data or unknown type - LWW fallback
                lww_merge(local_data, remote_data, metadata)
            }
        }
    }
}

/// LWW merge fallback
fn lww_merge(
    local_data: &[u8],
    remote_data: &[u8],
    metadata: &Metadata,
) -> Result<Vec<u8>, MergeError> {
    // Compare timestamps - remote wins if newer or equal
    let local_ts = metadata.updated_at();
    // Assume remote timestamp is in the remote metadata
    // For now, remote wins on tie (consistent with existing behavior)
    Ok(remote_data.to_vec())
}
```

### Root State Merging

The root state (`#[app::state] struct MyApp`) is **always custom**:

```rust
#[app::state]
struct MyApp {
    // These are built-in CRDTs
    counter: Counter,
    map: UnorderedMap<String, String>,
    
    // This is a custom type
    game: MyGameState,
}

// The ROOT STRUCT itself is custom - needs WASM merge
impl Mergeable for MyApp {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // App defines how to merge the overall state
        self.counter.merge(&other.counter)?;
        self.map.merge(&other.map)?;
        self.game.merge(&other.game)?;  // Custom merge
        Ok(())
    }
}
```

When the ROOT entities conflict (same ID, different content), we MUST call WASM:

```rust
fn merge_root_state(
    local: &[u8],
    remote: &[u8],
    wasm_callback: &dyn WasmMergeCallback,
) -> Result<Vec<u8>, MergeError> {
    // Root state is always custom - must use WASM
    wasm_callback.merge_root_state(local, remote)
}
```

### Collections Auto-Set Type

```rust
// Counter sets its type on creation
impl<S: StorageAdaptor> Counter<S> {
    pub fn new() -> Self {
        let mut element = Element::new();
        element.metadata_mut().crdt_type = Some(CrdtType::Counter);
        Self { element, counts: BTreeMap::new() }
    }
}

// UnorderedMap sets its type on creation
impl<K, V, S: StorageAdaptor> UnorderedMap<K, V, S> {
    pub fn new() -> Self {
        let mut element = Element::new();
        element.metadata_mut().crdt_type = Some(CrdtType::UnorderedMap);
        Self { element, entries: BTreeMap::new(), _phantom: PhantomData }
    }
}

// Custom types set via macro
#[app::state]  // Macro generates:
struct MyApp { /*...*/ }
// element.metadata_mut().crdt_type = Some(CrdtType::Custom {
//     type_name: "MyApp".to_string(),
// });
```

### Enforcing CRDT-Only State (Compile-Time)

The `#[app::state]` macro MUST reject non-CRDT fields:

```rust
// VALID: All fields are CRDTs
#[app::state]
struct MyApp {
    scores: Counter,                        // Built-in CRDT
    items: UnorderedMap<String, String>,    // Built-in CRDT
    name: LwwRegister<String>,              // Wrapped scalar
    config: LwwRegister<MyConfig>,          // Wrapped custom type
    game: MyGameState,                      // Custom Mergeable
}

// ❌ COMPILE ERROR: Raw scalars not allowed
#[app::state]
struct BadApp {
    name: String,        // ERROR: Use LwwRegister<String>
    count: u64,          // ERROR: Use LwwRegister<u64> or Counter
    data: Vec<u8>,       // ERROR: Use Vector<u8>
}
```

**Macro enforcement logic:**
```rust
// In #[app::state] macro
fn validate_field_type(ty: &Type) -> Result<(), CompileError> {
    if is_builtin_crdt(ty) {
        Ok(())  // Counter, UnorderedMap, Vector, etc.
    } else if is_lww_register(ty) {
        Ok(())  // LwwRegister<T> wraps any type
    } else if implements_mergeable(ty) {
        Ok(())  // Custom Mergeable type
    } else {
        Err(CompileError::new(
            format!(
                "Field type `{}` is not a CRDT. Wrap with LwwRegister<{}> or implement Mergeable.",
                ty, ty
            )
        ))
    }
}
```

This ensures **all state converges** - no silent data loss from LWW on non-CRDT types.

### The Generic Type Problem - SOLVED

**Question**: For `UnorderedMap<K, V>`, don't we need to know K and V types?

**Answer**: NO! Each entity stores its own `crdt_type` in Metadata.

```
UnorderedMap<String, Counter> in storage:
│
├── Map Entity (id: 0x123)
│   └── metadata.crdt_type = Some(CrdtType::UnorderedMap)
│
├── Entry "alice" (id: 0x456, parent: 0x123)
│   └── metadata.crdt_type = Some(CrdtType::Counter)  ← Self-describing!
│
└── Entry "bob" (id: 0x789, parent: 0x123)
    └── metadata.crdt_type = Some(CrdtType::Counter)  ← Self-describing!
```

**Merge algorithm**:
```rust
fn merge_entity(local: &Entity, remote: &Entity) -> Result<Vec<u8>> {
    // Each entity knows its own type - no ABI needed!
    match &local.metadata.crdt_type {
        Some(CrdtType::UnorderedMap) => {
            // Merge map: iterate children, merge each by THEIR crdt_type
            for (local_child, remote_child) in children_pairs {
                merge_entity(local_child, remote_child)?;  // Recursive!
            }
        }
        Some(CrdtType::Counter) => {
            // Merge counter directly
            let mut local: Counter = deserialize(local.data)?;
            let remote: Counter = deserialize(remote.data)?;
            local.merge(&remote)?;
        }
        // ...
    }
}
```

**No ABI required!** The Merkle tree is self-describing - every entity carries its type.

### Performance Analysis

```
Merge Benchmark (1000 entities):

Built-in CRDTs (Counter, Map, etc.):
├── Conflicts: 100 entities
├── Merge time: 100 × 100ns = 10μs total
└── WASM calls: 0

Custom Mergeable Types:
├── Conflicts: 10 entities
├── Merge time: 10 × 10μs = 100μs total
└── WASM calls: 10

Root State Conflicts:
├── Conflicts: 1 (rare - only on concurrent root updates)
├── Merge time: 1 × 10μs = 10μs
└── WASM calls: 1

Total: ~120μs for 111 conflicts
Network RTT: ~50ms

Merge overhead: 0.24% of sync time
```

### Sync API with WASM Callback

```rust
impl SyncManager {
    /// Sync state with hybrid merge support
    pub async fn sync_with_peer(&self, peer: PeerId) -> Result<SyncResult> {
        let foreign_state = self.fetch_state(peer).await?;
        
        // Create WASM callback if we have a loaded module
        let wasm_callback: Option<Box<dyn WasmMergeCallback>> = 
            self.wasm_module.as_ref().map(|m| {
                Box::new(WasmMergeCallbackImpl::new(m)) as Box<dyn WasmMergeCallback>
            });
        
        // Compare trees with hybrid merge
        let (local_actions, remote_actions) = Interface::compare_trees_full_with_merge(
            self.root_id,
            &foreign_state.index,
            &foreign_state.data,
            wasm_callback.as_deref(),
        )?;
        
        // Apply merged actions
        for action in local_actions {
            Interface::apply_action(&action)?;
        }
        
        // Send remote's needed actions
        self.send_actions(peer, remote_actions).await?;
        
        Ok(SyncResult::Completed)
    }
}
```

### Migration Path

| Phase | Change | Backwards Compatible? |
|-------|--------|----------------------|
| 1 | Add `crdt_type: Option<CrdtType>` to Metadata | Yes (Optional field) |
| 2 | Collections auto-set crdt_type on creation | Yes (Additive) |
| 3 | `#[app::state]` macro sets Custom type | Yes (Additive) |
| 4 | Tree comparison uses crdt_type for dispatch | Yes |
| 5 | Add WasmMergeCallback trait | Yes (Optional) |
| 6 | SyncManager creates callback from WASM module | Yes |
| 7 | Deprecate ResolutionStrategy | ⚠️ Migration needed |

**Note**: No ABI required! Each entity stores its own `crdt_type` in Metadata - the tree is self-describing.

### Summary: Why This Architecture

| Aspect | Old (ResolutionStrategy) | New (Hybrid CrdtType) |
|--------|--------------------------|----------------------|
| Built-in CRDT merge | LWW only (data loss!) | Proper CRDT merge |
| Custom type merge | Not supported | Via WASM callback |
| Performance | N/A | ~100ns built-in, ~10μs custom |
| WASM dependency | Required for all | Only for custom types |
| Type safety | None | Compile-time for built-in |
| Extensibility | None | App can define merge logic |

---

## Appendix B: Protocol Selection Matrix

### When to Use Each Protocol

| Protocol | Trigger Conditions | Best For | Avoid When |
|----------|-------------------|----------|------------|
| **DeltaSync** | Missing < 10 deltas, parents known | Real-time updates, small gaps | Fresh nodes, large gaps |
| **HashBasedSync** | Divergence 10-50%, depth any | General-purpose catch-up | 100% divergence (fresh node) |
| **BloomFilterSync** | Entities > 50, divergence < 10% | Large trees with tiny diff | Small trees, high divergence |
| **SubtreePrefetchSync** | Depth > 3, divergence < 20% | Deep hierarchies, localized changes | Shallow trees, scattered changes |
| **LevelWiseSync** | Depth ≤ 2 | Wide shallow trees | Deep hierarchies |
| **SnapshotSync** | Fresh node OR divergence > 50% | Bootstrap, major divergence | Tiny diffs (wasteful) |
| **CompressedSnapshotSync** | Entities > 100, fresh node | Large state bootstrap | Small state, low bandwidth |

### Protocol Selection Flowchart

```
                    ┌─────────────────────┐
                    │ Start Sync Decision │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │ root_hash matches?  │
                    └──────────┬──────────┘
                        Yes │      │ No
                            │      │
                    ┌───────▼──┐   │
                    │ NO SYNC  │   │
                    └──────────┘   │
                               ┌───▼───────────────┐
                               │ Has local state?  │
                               └───────┬───────────┘
                                No │       │ Yes
                                   │       │
                        ┌──────────▼───┐   │
                        │ SNAPSHOT     │   │
                        │ (compressed  │   │
                        │  if >100)    │   │
                        └──────────────┘   │
                                       ┌───▼───────────────┐
                                       │ Estimate          │
                                       │ divergence ratio  │
                                       └───────┬───────────┘
                                               │
                    ┌──────────────────────────┼──────────────────────────┐
                    │                          │                          │
              >50%  │                    10-50%│                     <10% │
                    │                          │                          │
           ┌────────▼────────┐      ┌──────────▼──────────┐    ┌─────────▼─────────┐
           │ SNAPSHOT        │      │ Check tree shape    │    │ BLOOM_FILTER      │
           └─────────────────┘      └──────────┬──────────┘    │ (if entities >50) │
                                               │               └───────────────────┘
                              ┌────────────────┼────────────────┐
                              │                │                │
                        depth>3         depth≤2          default
                              │                │                │
                     ┌────────▼────────┐ ┌─────▼─────┐ ┌────────▼────────┐
                     │ SUBTREE_PREFETCH│ │ LEVEL_WISE│ │ HASH_BASED      │
                     └─────────────────┘ └───────────┘ └─────────────────┘
```

---

## Appendix B.2: Eventual Consistency Guarantees

### How We Ensure All Nodes Converge

#### 1. Merkle Root Hash Invariant

**Guarantee**: After successful bidirectional sync, `root_hash(A) == root_hash(B)`

```
Before Sync:                 After Sync:
  Node A: [hash_a]             Node A: [hash_final]
  Node B: [hash_b]             Node B: [hash_final]
  
  hash_a ≠ hash_b              hash_final == hash_final ✓
```

#### 2. Multi-Node Convergence (Gossip)

With N > 2 nodes, pairwise sync eventually converges:

```
Time T0:
  A: [h1]  B: [h2]  C: [h3]  (all different)

Time T1: A syncs with B
  A: [h12] B: [h12] C: [h3]

Time T2: B syncs with C  
  A: [h12] B: [h123] C: [h123]

Time T3: A syncs with B (or C)
  A: [h123] B: [h123] C: [h123]  ✓ All converged
```

**Convergence Bound**: O(log N) sync rounds with random pairwise selection.

#### 3. Conflict Resolution Determinism

Same inputs → Same output (deterministic merge):

```rust
// Given same conflict data, all nodes make same decision
let result_a = resolve_conflict(local_data, foreign_data, strategy);
let result_b = resolve_conflict(local_data, foreign_data, strategy);
assert_eq!(result_a, result_b);  // Always true
```

**Strategies and their determinism:**

| Strategy | Deterministic? | Tie-breaker |
|----------|---------------|-------------|
| LastWriteWins | Yes | HLC timestamp, then data bytes |
| FirstWriteWins | Yes | HLC timestamp |
| MaxValue | Yes | Byte comparison |
| MinValue | Yes | Byte comparison |
| Manual | ⚠️ Requires app logic | App-defined |

#### 4. Causal Consistency via DAG

Deltas are applied in causal order:

```
Delta D3 (parents: [D1, D2])
    ↓
Cannot apply D3 until D1 AND D2 are applied
    ↓
Guarantees causal consistency
```

---

## Appendix C: Delta Pruning

### The Problem

Without pruning, delta history grows forever:
- Genesis → Delta1 → Delta2 → ... → Delta1000000
- New nodes must process ALL deltas (inefficient)
- Storage grows unbounded

### The Solution: Checkpoints

```rust
pub struct Checkpoint {
    /// Unique checkpoint ID
    pub id: [u8; 32],
    
    /// Root hash at checkpoint time
    pub root_hash: [u8; 32],
    
    /// HLC timestamp when created
    pub timestamp: HybridTimestamp,
    
    /// Full state snapshot
    pub snapshot: Snapshot,
    
    /// Last delta ID included in this checkpoint
    pub last_delta_id: [u8; 32],
    
    /// Signatures from N/M nodes (quorum attestation)
    pub attestations: Vec<NodeAttestation>,
}
```

### Checkpoint Creation Protocol

```
1. Leader proposes checkpoint at delta height H
2. Nodes verify their state matches proposed root_hash
3. Nodes sign attestation if state matches
4. Once quorum (e.g., 2/3) attestations collected:
   - Checkpoint is finalized
   - Deltas before H can be pruned
5. New nodes can start from checkpoint instead of genesis
```

### Pruning Safety

**Critical Invariant**: Only prune deltas if:
1. Checkpoint exists with root_hash matching current state
2. Quorum of nodes attested to the checkpoint
3. All nodes have received the checkpoint

```rust
impl CheckpointStore {
    fn can_prune_delta(&self, delta: &CausalDelta, checkpoint: &Checkpoint) -> bool {
        // Delta is before checkpoint
        delta.hlc < checkpoint.timestamp
            // AND checkpoint is finalized
            && checkpoint.attestations.len() >= QUORUM_SIZE
            // AND we have the checkpoint snapshot
            && self.has_checkpoint(&checkpoint.id)
    }
}
```

### Relationship with State Sync

| Scenario | Bootstrap From |
|----------|---------------|
| Has checkpoint | Checkpoint snapshot + deltas after checkpoint |
| No checkpoint | Genesis + all deltas OR peer snapshot |

---

## Appendix D: Edge Cases & Missing Pieces

### Edge Case 1: Concurrent Sync + Modifications

**Problem**: Node A is syncing from B while C sends new deltas.

**Solution**: Delta buffering (see Section 5)

```
During Sync:
  [Incoming deltas] → Buffer
  [Sync state] → Apply directly
  
After Sync:
  [Buffer] → Trigger DAG sync → Apply missing deltas
```

**Checkpoint**: CP-17 (Deltas received during state sync are buffered)

### Edge Case 1b: Concurrent Writes Creating Divergent Branches

**Problem**: Two nodes apply deltas concurrently, creating branches. When deltas propagate:
- D2a expects hash based on Node A's state
- D2b expects hash based on Node B's state  
- Applying D2b on Node A fails: `RootHashMismatch`

**Solution**: Smart concurrent branch detection

```rust
// Detect merge scenario
let is_merge = current_root != delta.expected_root 
    && parent_hash != Some(current_root);

if is_merge {
    // Use CRDT merge instead of direct apply
    sync_trees_with_callback(actions, merge_callback);
}
```

**Checkpoint**: CP-16 (Snapshot on initialized node uses CRDT merge)

### Edge Case 2: Partial Sync Failure

**Problem**: Sync fails midway (network error, node crash).

**Solution**: Atomic sync with rollback

```rust
pub struct SyncTransaction {
    /// Original state before sync started
    rollback_snapshot: Snapshot,
    
    /// Partial state received so far
    partial_state: PartialState,
    
    /// Has sync completed successfully?
    committed: bool,
}

impl Drop for SyncTransaction {
    fn drop(&mut self) {
        if !self.committed {
            // Rollback to original state
            apply_snapshot_unchecked(&self.rollback_snapshot);
        }
    }
}
```

### Edge Case 3: Byzantine/Malicious Nodes

**Problem**: Node sends tampered data.

**Solution**: Cryptographic verification (already implemented)

| Attack | Defense |
|--------|---------|
| Tampered entity data | Hash verification fails |
| Tampered root hash | Computed root ≠ claimed root |
| Replay old snapshot | HLC timestamp check |
| Forge attestations | Signature verification |

### Edge Case 4: Clock Skew

**Problem**: Node clocks are significantly different.

**Solution**: HLC bounds + peer clock sync

```rust
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(60);

fn validate_delta_timestamp(delta: &CausalDelta, local_hlc: &HybridTimestamp) -> bool {
    let drift = delta.hlc.physical_diff(local_hlc);
    drift < MAX_CLOCK_SKEW
}
```

### Edge Case 5: Large Entities

**Problem**: Single entity is huge (e.g., 100MB blob).

**Solution**: Chunked transfer with streaming

```rust
pub enum SyncMessage {
    // ... existing messages ...
    
    /// Large entity transferred in chunks
    EntityChunk {
        id: Id,
        chunk_index: u32,
        total_chunks: u32,
        data: Vec<u8>,
        chunk_hash: [u8; 32],
    },
}
```

### Edge Case 6: Tombstone Accumulation

**Problem**: Deleted entities leave tombstones forever.

**Solution**: Tombstone TTL + garbage collection

```rust
pub struct Tombstone {
    pub deleted_at: HybridTimestamp,
    pub ttl: Duration,  // e.g., 30 days
}

fn should_gc_tombstone(tombstone: &Tombstone, now: HybridTimestamp) -> bool {
    now.physical_time() > tombstone.deleted_at.physical_time() + tombstone.ttl
}
```

**GC Safety**: Only GC tombstones after:
1. TTL expired
2. All active nodes have seen the deletion
3. Checkpoint created after deletion

### Edge Case 7: Network Partition Healing

**Problem**: Two partitions evolved independently, now reconnecting.

```
Partition 1: A, B → root_hash_1 (1000 entities)
Partition 2: C, D → root_hash_2 (1000 entities)

After heal: 4 nodes, 2 different states
```

**Solution**: Merge reconciliation protocol

```rust
fn heal_partition(
    partition1_root: [u8; 32],
    partition2_root: [u8; 32],
) -> HealingStrategy {
    // Compare entity counts
    let p1_count = get_entity_count(partition1_root);
    let p2_count = get_entity_count(partition2_root);
    
    // If one partition has significantly more state, it likely has more truth
    // But we still need bidirectional merge
    
    HealingStrategy::BidirectionalMerge {
        // Sync partition1 → partition2
        // Then sync partition2 → partition1 (updated)
        // Repeat until convergence
    }
}
```

### Edge Case 8: Schema Evolution

**Problem**: Entity format changes between versions.

**Solution**: Version tagging + migration

```rust
pub struct EntityEnvelope {
    pub version: u32,
    pub data: Vec<u8>,
}

fn deserialize_entity(envelope: &EntityEnvelope) -> Result<Entity> {
    match envelope.version {
        1 => deserialize_v1(&envelope.data),
        2 => deserialize_v2(&envelope.data),
        v => Err(UnknownVersion(v)),
    }
}
```

---

## Appendix E: Open Design Questions

The following design questions are deferred to future CIPs or implementation decisions:

### Checkpoint Protocol (Future CIP)

| Question | Considerations |
|----------|----------------|
| Checkpoint frequency | Too frequent increases storage/network cost; too rare increases bootstrap time. RECOMMENDED: configurable, default 1000 deltas OR 1 hour. |
| Quorum size for attestation | 2/3+1 for Byzantine tolerance; simple majority for crash tolerance only. RECOMMENDED: configurable per context. |
| Checkpoint storage format | Full snapshot vs incremental diff from previous checkpoint. |

### Tombstone Garbage Collection (Future CIP)

| Question | Considerations |
|----------|----------------|
| Tombstone TTL | Too short enables resurrection attacks; too long causes storage bloat. RECOMMENDED: 30 days default, configurable. |
| GC safety conditions | Must ensure all active nodes have seen deletion before GC. |

### Future Extensions

| Extension | Benefit | Complexity |
|-----------|---------|------------|
| Merkle proof for single entity | Verify entity without full state | Low |
| Incremental checkpoint updates | Avoid regenerating full snapshot | Medium |
| Probabilistic sync skip | Skip sync if bloom filter shows no diff | Low |
| Adaptive sync frequency | Sync more often during high activity | Medium |
| Large entity chunked transfer | Handle entities > 1MB | Medium |

## References

- [CRDT Literature](https://crdt.tech/)
- [Merkle Trees](https://en.wikipedia.org/wiki/Merkle_tree)
- [Hybrid Logical Clocks](https://cse.buffalo.edu/tech-reports/2014-04.pdf)
- [EIP-1 Format](https://eips.ethereum.org/EIPS/eip-1)

## Copyright

Copyright and related rights waived via [CC0](https://creativecommons.org/publicdomain/zero/1.0/).

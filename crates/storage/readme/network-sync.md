# Network Synchronization Protocols

This document describes the Merkle tree synchronization protocols implemented for efficient state synchronization between distributed nodes.

## Overview

When two nodes need to synchronize their state, they must efficiently determine:
1. **What differs** between their Merkle trees
2. **How to transfer** only the necessary data
3. **How to resolve conflicts** when both have changes

The storage layer uses a hierarchical Merkle tree where each entity has:
- **`own_hash`**: Hash of the entity's own data
- **`full_hash`**: Hash of own data + all descendants (for quick subtree comparison)

## Design Goals

1. **Minimize round trips** - Batch requests when possible
2. **Minimize data transfer** - Only send what's different  
3. **Choose optimal protocol** - Different scenarios need different approaches
4. **Support conflict resolution** - Use configurable resolution strategies

## Synchronization Protocols

### Protocol 1: Hash-Based Comparison (Baseline)

The standard recursive Merkle tree comparison protocol.

```
Local                          Remote
  |                               |
  |------- Request root hash ---->|
  |<------ Root hash -------------|
  |                               |
  | (if hashes differ)            |
  |------- Request entities ----->|  ← Batched by level
  |<------ Entities + hashes -----|
  |                               |
  | (for each differing child)    |
  |------- Request children ----->|
  |<------ Child data ------------|
```

**Best for**: General incremental synchronization
**Trade-offs**: Multiple round trips for deep trees

### Protocol 2: Snapshot Transfer

Transfer the entire state in a single request.

```
Local                          Remote
  |                               |
  |------- Request snapshot ----->|
  |<------ Full snapshot ---------|
  |                               |
  | (apply snapshot locally)      |
```

**Best for**: Fresh nodes (bootstrap), large divergence (>50%)
**Trade-offs**: High bandwidth for large states

### Protocol 3: Subtree Prefetch

When detecting a differing subtree, fetch the entire subtree at once.

```
Local                          Remote
  |                               |
  |------- Request root + summary -->|
  |<------ Hash + child hashes ------|
  |                               |
  | (compare child hashes locally)|
  |                               |
  |------- Request subtree A ---->|  ← Entire differing subtree
  |<------ All entities in A -----|  ← Single response
```

**Best for**: Deep trees with localized changes (e.g., one branch modified)
**Trade-offs**: May over-fetch if only leaf changed

### Protocol 4: Bloom Filter Sync

Use probabilistic data structure for quick diff detection.

```
Local                          Remote
  |                               |
  |------- Send Bloom filter ---->|  ← Compact (~1KB for 1000 items)
  |<------ Missing entities ------|  ← Only what's definitely missing
```

**How it works**:
1. Local builds a Bloom filter of all entity IDs
2. Remote checks each of its IDs against the filter
3. IDs not in filter are definitely missing → send them
4. IDs in filter might be present → verify hash if needed

**Best for**: Large trees with small diffs (<10%)
**Trade-offs**: False positives require hash verification

### Protocol 5: Level-Wise Sync

Synchronize one depth level at a time (breadth-first).

```
Local                          Remote
  |                               |
  |------- Request level 0 ------>|
  |<------ Root entity -----------|
  |                               |
  |------- Request level 1 ------>|  ← All children of differing parents
  |<------ Level 1 entities ------|
  |                               |
  |------- Request level 2 ------>|
  |<------ Level 2 entities ------|
```

**Best for**: Wide, shallow trees (many siblings, few levels)
**Trade-offs**: Fixed round trips = tree depth

### Protocol 6: Compressed Snapshot

Snapshot transfer with compression for bandwidth-constrained networks.

```
Local                          Remote
  |                               |
  |--- Request compressed snap -->|
  |<-- Compressed data -----------|  ← ~60% smaller with LZ4/zstd
```

**Best for**: Fresh nodes on slow networks, large states
**Trade-offs**: CPU overhead for compression/decompression

## Protocol Selection (Smart Adaptive Sync)

The `SmartAdaptiveSync` automatically selects the optimal protocol:

```
┌─────────────────────────────────────────────────────────────┐
│                    Protocol Selection                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Fresh node (no local data)?                                │
│    └─ YES → Snapshot (or CompressedSnapshot if >100 items)  │
│                                                             │
│  Large divergence (>50% different)?                         │
│    └─ YES → Snapshot                                        │
│                                                             │
│  Deep tree (depth >3) with few subtrees (<10)?              │
│    └─ YES → SubtreePrefetch                                 │
│                                                             │
│  Large tree (>50 items) with small diff (<10%)?             │
│    └─ YES → BloomFilter                                     │
│                                                             │
│  Wide shallow tree (depth ≤2, many children)?               │
│    └─ YES → LevelWise                                       │
│                                                             │
│  Default → HashComparison                                   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Efficiency Comparison

Benchmark results from test scenarios:

| Scenario | Protocol | Round Trips | Bytes Transferred |
|----------|----------|-------------|-------------------|
| Fresh node (50 entities) | Hash-based | 2 | 240 |
| Fresh node (50 entities) | Snapshot | 1 | 8,758 |
| Fresh node (50 entities) | Compressed | 1 | **1,354** (84% savings) |
| 5% difference (100 entities) | Hash-based | 3 | 1,250 |
| 5% difference (100 entities) | Bloom filter | **1** | 1,186 |
| Deep localized change | Hash-based | 4 | 3,459 |
| Deep localized change | Subtree prefetch | **2** | 3,444 |

### Key Insights

1. **Fresh nodes**: Compressed snapshot saves ~85% bandwidth vs regular snapshot
2. **Small diffs**: Bloom filter reduces round trips by 66% (3→1)
3. **Localized changes**: Subtree prefetch cuts round trips by 50%
4. **Already synced**: All protocols detect this in 1 round trip

## Conflict Resolution

When entities differ, the system uses configurable `ResolutionStrategy`:

```rust
pub enum ResolutionStrategy {
    LastWriteWins,   // Default: newer timestamp wins
    FirstWriteWins,  // Older timestamp wins  
    MaxValue,        // Lexicographically greater value wins
    MinValue,        // Lexicographically smaller value wins
    Manual,          // Generate Compare action for manual resolution
}
```

Resolution is applied during `compare_trees_full()`:

```rust
// In compare_trees_full
if local_hash != remote_hash {
    let strategy = metadata.resolution;
    match strategy.resolve(local_data, local_metadata, remote_data, remote_metadata) {
        Some(true) => /* accept remote */,
        Some(false) => /* keep local */,
        None => /* generate Compare action for manual handling */,
    }
}
```

## Network Message Types

```rust
enum SyncMessage {
    // Basic protocol
    RequestRootHash,
    RootHashResponse { hash, has_data },
    RequestEntities { ids: Vec<Id> },
    EntitiesResponse { entities: Vec<(Id, data, comparison)> },
    
    // Snapshot
    RequestSnapshot,
    SnapshotResponse { snapshot },
    RequestCompressedSnapshot,
    CompressedSnapshotResponse { compressed_data, original_size },
    
    // Optimized
    RequestRootHashWithSummary,
    RootHashWithSummaryResponse { hash, entity_count, depth, child_hashes },
    RequestSubtree { root_id, max_depth },
    SubtreeResponse { entities, truncated },
    SendBloomFilter { filter, local_root_hash },
    BloomFilterDiffResponse { missing_entities, already_synced },
    RequestLevel { level, parent_ids },
    LevelResponse { children },
}
```

## Bloom Filter Implementation

The Bloom filter provides probabilistic set membership testing:

```rust
struct BloomFilter {
    bits: Vec<u8>,      // Bit array
    num_hashes: usize,  // Number of hash functions (k)
    num_items: usize,   // Items inserted
}
```

**Parameters** (automatically calculated):
- **Size (m)**: `m = -n * ln(p) / (ln(2)²)` where n=expected items, p=false positive rate
- **Hash count (k)**: `k = (m/n) * ln(2)`

**Default**: 1% false positive rate, minimum 64 bits

## Usage Example

```rust
// Automatic protocol selection
let mut channel = NetworkChannel::new();
let (method, stats) = SmartAdaptiveSync::sync::<LocalStorage, RemoteStorage>(&mut channel)?;

println!("Used protocol: {:?}", method);
println!("Round trips: {}", stats.round_trips);
println!("Bytes transferred: {}", stats.total_bytes());

// Manual protocol selection
let mut channel = NetworkChannel::new();
let (actions, stats) = BloomFilterSync::sync::<Local, Remote>(&mut channel)?;
apply_actions_to::<Local>(actions)?;
```

## Implementation Files

- `crates/storage/src/tests/network_sync.rs` - Protocol implementations and tests
- `crates/storage/src/tests/tree_sync.rs` - Local tree sync tests (no network simulation)
- `crates/storage/src/interface.rs` - `compare_trees_full()`, `sync_trees()`
- `crates/storage/src/snapshot.rs` - Snapshot generation and application
- `crates/storage/src/entities.rs` - `ResolutionStrategy` enum

## Future Improvements

1. **Delta encoding**: Send byte-level diffs for updates instead of full data
2. **Merkle Patricia Trie**: More efficient for sparse key spaces
3. **Pipelining**: Start processing response while next request is in flight
4. **Checkpointing**: Remember last sync point to skip unchanged subtrees
5. **Adaptive batch sizing**: Adjust batch size based on network latency

## References

- [Merkle Trees](https://en.wikipedia.org/wiki/Merkle_tree)
- [Bloom Filters](https://en.wikipedia.org/wiki/Bloom_filter)
- [Anti-Entropy Protocols](https://en.wikipedia.org/wiki/Gossip_protocol)
- [CRDTs and Eventual Consistency](https://crdt.tech/)

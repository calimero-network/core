# Sync Protocol Documentation Index

**Branch**: `test/tree_sync`  
**Status**: Ready for Review  
**Last Updated**: February 1, 2026

---

## Quick Start for Reviewers

This branch implements a **hybrid state synchronization protocol** that combines delta-based and state-based approaches. Start here:

| Document | Purpose | Read Time |
|----------|---------|-----------|
| [CIP-sync-protocol.md](./CIP-sync-protocol.md) | **Main specification** - full protocol design | 30 min |
| [network-sync.md](./network-sync.md) | High-level overview of sync strategies | 10 min |
| [TECH-DEBT-SYNC-2026-01.md](./TECH-DEBT-SYNC-2026-01.md) | Implementation status & known issues | 5 min |

---

## What This Branch Implements

### Core Features

| Feature | Status | Key Files |
|---------|--------|-----------|
| **Protocol Negotiation** | ✅ Done | `sync/manager.rs` |
| **Hash-Based Comparison** | ✅ Done | `sync/tree_sync.rs` |
| **Bloom Filter Sync** | ✅ Done | `sync/tree_sync.rs` |
| **Subtree Prefetch** | ✅ Done | `sync/tree_sync.rs` |
| **Level-Wise Sync** | ✅ Done | `sync/tree_sync.rs` |
| **Snapshot Sync** | ✅ Done | `sync/snapshot.rs` |
| **CRDT Merge in Tree Sync** | ✅ Done | `sync/tree_sync.rs`, `storage/interface.rs` |
| **Checkpoint Deltas** | ✅ Done | `dag/src/lib.rs` |
| **Parallel Dialing** | ✅ Done | `sync/dial_tracker.rs` |

### Critical Bug Fixes

1. **CRDT Merge in State Sync** - Tree sync was using LWW instead of proper CRDT merge. Now correctly dispatches based on `crdt_type` in entity metadata.

2. **Network Event Delivery** - `LazyRecipient` was silently dropping messages. Replaced with explicit mpsc channel.

3. **Snapshot Boundary** - Replaced fake delta stubs with proper `DeltaKind::Checkpoint`.

4. **Metadata Persistence** (Feb 1) - Tree sync was writing entity values but NOT `EntityIndex` with `crdt_type`. Now calls `Index::persist_metadata_for_sync()`.

5. **Bloom Filter Hash** (Feb 1) - `sync_protocol.rs` used FNV-1a but `dag/lib.rs` used SipHash. Now both use FNV-1a.

6. **Buffered Delta Replay** (Feb 1) - `BufferedDelta` was missing `nonce`, `author_id`, `root_hash`, `events` needed for replay after snapshot.

7. **Protocol Version** (Feb 1) - Wire format changed but HybridSync was still v1. Bumped to **HybridSync v2**.

---

## Document Map

### Design & Specification

| Document | Description |
|----------|-------------|
| **[CIP-sync-protocol.md](./CIP-sync-protocol.md)** | Full protocol specification with message formats, negotiation flow, and CRDT merge semantics |
| [network-sync.md](./network-sync.md) | Overview of sync strategies (hash comparison, Bloom filter, snapshot, etc.) |
| [merging.md](./merging.md) | CRDT merge semantics and the `Mergeable` trait |

### Architecture & Decisions

| Document | Description |
|----------|-------------|
| [RFC-ACTIX-NETWORK-ARCHITECTURE.md](./RFC-ACTIX-NETWORK-ARCHITECTURE.md) | Discussion of `LazyRecipient` issue and proposed migration away from Actix |
| [design-decisions.md](./design-decisions.md) | Storage layer design rationale |
| [architecture.md](./architecture.md) | Storage architecture overview |

### Operations & Monitoring

| Document | Description |
|----------|-------------|
| [PRODUCTION-MONITORING.md](./PRODUCTION-MONITORING.md) | Prometheus alerts, Grafana dashboards, and SLIs for sync operations |
| [TECH-DEBT-SYNC-2026-01.md](./TECH-DEBT-SYNC-2026-01.md) | Implementation status, resolved issues, and future optimizations |

### Other Storage Docs

| Document | Description |
|----------|-------------|
| [collections.md](./collections.md) | CRDT collections (UnorderedMap, Counter, etc.) |
| [nesting.md](./nesting.md) | Nested CRDT patterns |
| [performance.md](./performance.md) | Performance characteristics |
| [migration.md](./migration.md) | State migration guide |

---

## Key Code Locations

```
crates/
├── dag/src/lib.rs                    # DeltaKind::Checkpoint
├── node/
│   ├── primitives/src/sync.rs        # Wire protocol messages
│   └── src/sync/
│       ├── manager.rs                # SyncManager orchestration
│       ├── tree_sync.rs              # All tree sync strategies
│       ├── snapshot.rs               # Snapshot sync
│       ├── dial_tracker.rs           # Parallel dialing
│       └── peer_finder.rs            # Peer selection
├── storage/
│   └── src/interface.rs              # merge_by_crdt_type_with_callback()
└── apps/sync-test/                   # Comprehensive test app
```

---

## Testing

### Run Comprehensive Sync Test

```bash
# Build
cargo build --release -p merod -p meroctl
./apps/sync-test/build.sh

# Run E2E test
merobox bootstrap run --no-docker --binary-path ./target/release/merod \
  apps/sync-test/workflows/comprehensive-sync-test.yml
```

### Run Existing Benchmarks

```bash
# Disjoint writes (baseline)
merobox bootstrap run --no-docker --binary-path ./target/release/merod \
  workflows/sync/bench-3n-10k-disjoint.yml

# Conflict resolution (LWW)
merobox bootstrap run --no-docker --binary-path ./target/release/merod \
  workflows/sync/bench-3n-50k-conflicts.yml

# Late joiner (fresh node sync)
merobox bootstrap run --no-docker --binary-path ./target/release/merod \
  workflows/sync/bench-3n-late-joiner.yml
```

---

## Review Checklist

For reviewers, please verify:

- [ ] **Protocol Negotiation**: `SyncHandshake` → `SyncHandshakeResponse` flow
- [ ] **CRDT Merge**: `TreeLeafData` includes `Metadata` with `crdt_type`
- [ ] **Checkpoint Deltas**: `DeltaKind::Checkpoint` for snapshot boundaries
- [ ] **Parallel Dialing**: `FuturesUnordered` in `perform_interval_sync()`
- [ ] **Bloom Filter**: Response includes `TreeLeafData` (not raw bytes)
- [ ] **Tests Pass**: All benchmark workflows pass

---

## Future Work (Not in This PR)

| Item | Priority | Notes |
|------|----------|-------|
| Payload Compression | P1 | zstd for large transfers |
| WASM Custom Merge | P2 | `__calimero_merge` export |
| Actix Migration | P2 | Replace with pure tokio |
| Delta Pruning | P3 | Compact old deltas |

See [TECH-DEBT-SYNC-2026-01.md](./TECH-DEBT-SYNC-2026-01.md) for details.

---

## Questions?

Open an issue or comment on the PR. Key areas for discussion:

1. **Should we migrate away from Actix?** See [RFC-ACTIX-NETWORK-ARCHITECTURE.md](./RFC-ACTIX-NETWORK-ARCHITECTURE.md)
2. **Compression strategy** - zstd vs lz4, threshold values
3. **PR structure** - This branch is large; we plan to split into smaller PRs


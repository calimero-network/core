# Sync Protocol Documentation Index

**Branch**: `test/tree_sync`  
**Status**: Ready for Review  
**Last Updated**: February 1, 2026

---

## Quick Start for Reviewers

This branch implements a **hybrid state synchronization protocol** that combines delta-based and state-based approaches.

| Document | Purpose | Read Time |
|----------|---------|-----------|
| [CIP-sync-protocol.md](./CIP-sync-protocol.md) | **Protocol specification** - message formats, negotiation, CRDT merge | 25 min |
| [ARCHITECTURE-DECISIONS.md](./ARCHITECTURE-DECISIONS.md) | **Why we built it this way** - key implementation decisions | 10 min |
| [POC-IMPLEMENTATION-NOTES.md](./POC-IMPLEMENTATION-NOTES.md) | Branch-specific bugs, fixes, and status | 5 min |

---

## Document Structure

We've organized documentation by purpose:

### 1. Protocol Specification (CIP)

**[CIP-sync-protocol.md](./CIP-sync-protocol.md)** - The formal specification. Contains:
- Message formats and wire protocol
- Negotiation rules and state machines
- CRDT merge semantics
- Security considerations
- Backward compatibility

### 2. Architecture Decisions (Cookbook)

**[ARCHITECTURE-DECISIONS.md](./ARCHITECTURE-DECISIONS.md)** - Implementation choices. Contains:
- Why we chose FNV-1a for bloom hashes
- Why checkpoint deltas (not stubs)
- Why parallel dialing with sliding window
- Why HybridSync v2 breaking change
- Network event channel design

### 3. POC Implementation Notes

**[POC-IMPLEMENTATION-NOTES.md](./POC-IMPLEMENTATION-NOTES.md)** - Branch-specific details. Contains:
- Implementation phases
- Bugs discovered and fixed
- Performance findings
- Test results

---

## Supporting Documents

| Document | Description |
|----------|-------------|
| [network-sync.md](./network-sync.md) | High-level sync strategy overview |
| [RFC-ACTIX-NETWORK-ARCHITECTURE.md](./RFC-ACTIX-NETWORK-ARCHITECTURE.md) | Future: Migrate away from Actix |
| [PRODUCTION-MONITORING.md](./PRODUCTION-MONITORING.md) | Prometheus alerts, Grafana dashboards |
| [TECH-DEBT-SYNC-2026-01.md](./TECH-DEBT-SYNC-2026-01.md) | Detailed implementation status |

---

## Key Code Locations

```
crates/
├── dag/src/lib.rs                    # DeltaKind::Checkpoint, bloom hash
├── node/
│   ├── primitives/src/
│   │   ├── sync.rs                   # TreeLeafData, TreeNode
│   │   └── sync_protocol.rs          # SyncHandshake, BufferedDelta
│   └── src/sync/
│       ├── manager.rs                # SyncManager orchestration
│       ├── tree_sync.rs              # Tree sync strategies
│       ├── snapshot.rs               # Snapshot sync
│       ├── dial_tracker.rs           # Parallel dialing
│       └── peer_finder.rs            # Peer selection
├── storage/
│   └── src/
│       ├── interface.rs              # merge_by_crdt_type_with_callback()
│       └── index.rs                  # persist_metadata_for_sync()
└── apps/sync-test/                   # Comprehensive test app
```

---

## Review Checklist

- [ ] **CIP**: Message formats make sense
- [ ] **Architecture Decisions**: Decisions are justified
- [ ] **Code**: Key files implement the spec correctly

Quick validation:

```bash
# Build
cargo build --release -p merod

# Run unit tests
cargo test --package calimero-node --package calimero-storage

# Run E2E (if merobox available)
merobox bootstrap run --no-docker --binary-path ./target/release/merod \
  workflows/sync/three-node-sync.yml
```

---

## Future Work (Not in This PR)

| Item | Priority | Notes |
|------|----------|-------|
| Payload Compression | P1 | zstd for large transfers |
| WASM Custom Merge | P2 | `__calimero_merge` export |
| Actix Migration | P2 | Replace with pure tokio |
| Delta Pruning | P3 | Compact old deltas |

---

*For questions, comment on the PR.*

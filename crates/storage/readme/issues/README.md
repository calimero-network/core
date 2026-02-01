# Hybrid Sync Protocol - Implementation Issues

> **Source**: [CIP-sync-protocol.md](../CIP-sync-protocol.md)  
> **Reference**: [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)

## Overview

This folder contains implementation issues derived from the Hybrid State Synchronization Protocol CIP. Each issue is self-contained and can be worked on independently (respecting dependencies).

## Issue Index

### Foundation (Must be done first)

| Issue | Title | Priority | Depends On |
|-------|-------|----------|------------|
| [001](./001-crdt-type-metadata.md) | Add CrdtType to Entity Metadata | P0 | - |
| [002](./002-deterministic-entity-ids.md) | Deterministic Entity/Collection IDs | P0 | - |
| [003](./003-sync-handshake-messages.md) | Sync Handshake Protocol Messages | P0 | - |

### Core Protocol

| Issue | Title | Priority | Depends On |
|-------|-------|----------|------------|
| [004](./004-protocol-negotiation.md) | Protocol Negotiation & Selection | P0 | 003 |
| [005](./005-delta-sync.md) | Delta Sync Implementation | P1 | 003, 004 |
| [006](./006-delta-buffering.md) | Delta Buffering During State Sync | P0 | 003 |

### State-Based Sync Strategies

| Issue | Title | Priority | Depends On |
|-------|-------|----------|------------|
| [007](./007-hash-comparison-sync.md) | HashComparison Sync Strategy | P0 | 001, 003, 004 |
| [008](./008-bloom-filter-sync.md) | BloomFilter Sync Strategy | P1 | 007 |
| [009](./009-subtree-prefetch-sync.md) | SubtreePrefetch Sync Strategy | P2 | 007 |
| [010](./010-level-wise-sync.md) | LevelWise Sync Strategy | P2 | 007 |
| [011](./011-snapshot-sync.md) | Snapshot Sync (Fresh Nodes Only) | P1 | 001, 003 |

### CRDT Merge Architecture

| Issue | Title | Priority | Depends On |
|-------|-------|----------|------------|
| [012](./012-builtin-crdt-merge.md) | Built-in CRDT Merge in Storage Layer | P0 | 001 |
| [013](./013-wasm-merge-callback.md) | WASM Merge Callback for Custom Types | P1 | 012 |
| [014](./014-entity-transfer-metadata.md) | Entity Transfer with Metadata (TreeLeafData) | P0 | 001, 007 |

### Verification & Safety

| Issue | Title | Priority | Depends On |
|-------|-------|----------|------------|
| [015](./015-snapshot-verification.md) | Snapshot Cryptographic Verification | P0 | 011 |
| [016](./016-snapshot-merge-protection.md) | Snapshot Merge Protection (Invariant I5) | P0 | 011, 012 |

### Observability & Testing

| Issue | Title | Priority | Depends On |
|-------|-------|----------|------------|
| [017](./017-sync-metrics.md) | Sync Metrics & Observability | P2 | All |
| [018](./018-compliance-tests.md) | Compliance Test Suite | P1 | All |

## Suggested Implementation Order

```
Phase 1: Foundation
├── 001-crdt-type-metadata
├── 002-deterministic-entity-ids
└── 003-sync-handshake-messages

Phase 2: Core Protocol
├── 004-protocol-negotiation
├── 006-delta-buffering
└── 012-builtin-crdt-merge

Phase 3: Primary Sync Strategy
├── 007-hash-comparison-sync
├── 014-entity-transfer-metadata
└── 016-snapshot-merge-protection

Phase 4: Additional Strategies
├── 005-delta-sync
├── 008-bloom-filter-sync
├── 011-snapshot-sync
└── 015-snapshot-verification

Phase 5: Extensions
├── 009-subtree-prefetch-sync
├── 010-level-wise-sync
└── 013-wasm-merge-callback

Phase 6: Polish
├── 017-sync-metrics
└── 018-compliance-tests
```

## Labels

Use these labels when creating GitHub issues:

- `sync-protocol` - All sync-related issues
- `crdt` - CRDT merge functionality
- `storage` - Storage layer changes
- `network` - Network protocol changes
- `breaking` - Breaking wire protocol changes
- `P0`/`P1`/`P2` - Priority levels

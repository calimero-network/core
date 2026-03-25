# DAG-Based Group Governance

> Design for embedding group governance operations into the DAG infrastructure,
> providing causal ordering, offline catch-up, and temporal authorization.

## Problem

The current flat `nonce`-per-signer model for `SignedGroupOp` has fundamental issues:

1. **No causal ordering** — When admin A revokes member B's permission and member B
   concurrently creates a context, there's no way to determine which happened first.
   The nonce is per-signer so A's nonce=5 and B's nonce=3 are incomparable.

2. **No offline catch-up** — Gossip is fire-and-forget. If a node misses an op, it
   diverges permanently. The op log (added in this branch) helps but there's no
   protocol to exchange it.

3. **No merge semantics** — Two admins making concurrent changes (both online, both
   valid) can't express "I saw your change and mine" without a merge point.

4. **No temporal queries** — "Was this member authorized at the time this context was
   created?" is unanswerable without causal history.

## Solution: Reuse `CausalDelta<SignedGroupOp>`

The `calimero-dag` crate provides a generic `CausalDelta<T>` with:
- Content-addressed delta IDs
- Parent references for causal ordering (multiple parents = merge)
- Pending queue for out-of-order delivery
- Head tracking (tips with no children)
- `DeltaApplier<T>` trait for applying payloads to storage

We use `CausalDelta<SignedGroupOp>` to get all of this for free.

## Architecture

```
SignedGroupOp (existing)
       │
       ▼
CausalDelta<SignedGroupOp>    ◄── wraps op with: delta_id, parents, hlc
       │
       ├── DagStore<SignedGroupOp>     (in-memory topology, pending queue, heads)
       │
       ├── GroupGovernanceApplier      (impl DeltaApplier<SignedGroupOp>)
       │       │
       │       └── apply_local_signed_group_op()  ──► group_store (RocksDB)
       │
       ├── GroupOpLog (persistent)     (delta_id → borsh bytes in RocksDB)
       │
       └── Gossip / Sync
               │
               ├── BroadcastMessage::GroupGovernanceDelta  (gossip)
               │       carries: delta_id, parent_ids, SignedGroupOp borsh, hlc
               │
               └── Stream protocol: request missing deltas by ID
                       (same pattern as context DeltaRequest)
```

## Wire Format

### `SignedGroupOp` Changes (v2)

The `parent_op_hash: Option<[u8; 32]>` field becomes `parent_op_hashes: Vec<[u8; 32]>` 
to support merge deltas. The `nonce` field is kept for replay protection within a 
signer's stream but is no longer the primary ordering mechanism.

Schema version bumps to `SIGNED_GROUP_OP_SCHEMA_VERSION = 2`.

### `BroadcastMessage` New Variant

```rust
GroupGovernanceDelta {
    group_id: [u8; 32],
    delta_id: [u8; 32],
    parent_ids: Vec<[u8; 32]>,
    hlc: HybridTimestamp,
    /// borsh(SignedGroupOp) — the actual signed operation
    payload: Vec<u8>,
}
```

This replaces `SignedGroupOpV1` which carries no DAG metadata.

### Heartbeat

```rust
GroupStateHeartbeat {
    group_id: [u8; 32],
    dag_heads: Vec<[u8; 32]>,
    member_count: u32,
}
```

Peers compare `dag_heads`; if a peer has heads we lack, trigger sync.

## Key Design Decisions

### 1. Delta ID = Content Hash of SignedGroupOp

`delta_id = SHA-256(borsh(SignedGroupOp))` — this is already computed by
`SignedGroupOp::content_hash()`. The delta is content-addressed and
self-authenticating (signature inside).

### 2. Parents = Current DAG Heads at Signing Time

When a node signs a new group op, it reads the current `dag_heads` from
`GroupOpHead` and uses them as parents. This creates the causal link:
"I have seen and incorporated all ops up to these heads."

For a single-admin group, this is a linear chain. For multi-admin groups,
concurrent ops create branches that merge when either admin's next op
references both heads.

### 3. Authorization is Checked at Apply Time

The `DeltaApplier` calls `apply_local_signed_group_op()` which checks
admin/capability permissions against the **current** group state. Because
ops apply in topological order, a revocation that happens-before a
context creation will be applied first, correctly blocking the creation.

If ops are truly concurrent (neither is an ancestor of the other),
the DAG's topological sort + HLC tiebreaking determines order. For
governance this means: concurrent revocation and action by the same
member is a race, resolved deterministically across all nodes.

### 4. Nonce Kept for Per-Signer Dedup

The nonce remains as a lightweight filter: if we've already seen
`nonce <= last` for a signer, skip without full DAG lookup. The DAG's
`DuplicateDelta` error handles content-level dedup.

### 5. Sync via Delta Request (Same as Context)

Missing parents trigger `request_missing_deltas` over libp2p streams,
using `InitPayload::GroupDeltaRequest { group_id, delta_id }` and
`MessagePayload::GroupDeltaResponse { ... }`. This reuses the exact
same stream protocol pattern as context delta sync.

## Migration from v1

The `SignedGroupOpV1` gossip variant continues to be accepted for
backward compatibility during migration. V1 ops are wrapped in a
single-parent `CausalDelta` pointing to the current head.

The op log entries already persisted by the v1 code are compatible:
they can be loaded and inserted into the DAG on startup.

## Implementation Plan

### Phase 1: DAG Primitives (this PR)
- [x] Update `parent_op_hash` → `parent_op_hashes: Vec<[u8; 32]>`
- [x] Store `dag_heads` in `GroupOpHead` instead of single sequence
- [x] Integrate `DagStore<SignedGroupOp>` in group store
- [x] Implement `GroupGovernanceApplier`
- [x] Pending queue for out-of-order ops
- [x] Tests: concurrent ops, merge, out-of-order delivery

### Phase 2: Wire Protocol (follow-up)
- [ ] Add `BroadcastMessage::GroupGovernanceDelta`
- [ ] Update `network_event.rs` ingress to build `CausalDelta`
- [ ] Update `publish_signed_group_op` to include DAG metadata
- [ ] Add `GroupStateHeartbeat` broadcast
- [ ] Add `GroupDeltaRequest`/`GroupDeltaResponse` stream protocol

### Phase 3: Node Integration (follow-up)
- [ ] Per-group `DagStore` lifecycle in `ContextManager`
- [ ] Persist/reload DAG state across restarts
- [ ] Trigger delta request on missing parents
- [ ] Trigger sync on heartbeat divergence

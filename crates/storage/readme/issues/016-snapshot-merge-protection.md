# Issue 016: Snapshot Merge Protection (Invariant I5)

**Priority**: P0 (Data Safety Critical)  
**CIP Section**: §6.3 - Snapshot Usage Constraints  
**Invariant**: I5 (No Silent Data Loss)

## Summary

Implement safety mechanisms that prevent snapshot-based state overwrite on initialized nodes.

## The Problem

Snapshot sync on an initialized node would:
- Clear local state
- Apply remote state
- **Lose all local concurrent updates**

This violates CRDT convergence guarantees.

## Two Layers of Protection

### Layer 1: Protocol Selection (Automatic)

The protocol selection algorithm MUST NOT return Snapshot for initialized nodes:

```rust
fn select_protocol(local: &SyncHandshake, remote: &SyncHandshake) -> SyncProtocol {
    if !local.has_state {
        // Fresh node - Snapshot OK
        return SyncProtocol::Snapshot { ... };
    }
    
    // INITIALIZED NODE: Never use Snapshot
    // Even for >50% divergence, use HashComparison
    SyncProtocol::HashComparison { ... }
}
```

### Layer 2: Runtime Safety Check (Defense in Depth)

Even if a Snapshot is somehow selected (e.g., via CLI override), block it:

```rust
fn apply_sync_protocol(protocol: SyncProtocol) -> Result<()> {
    match protocol {
        SyncProtocol::Snapshot { .. } => {
            if storage.has_state() {
                warn!("SAFETY: Snapshot blocked for initialized node");
                // Fallback to HashComparison
                return apply_hash_comparison()?;
            }
            apply_snapshot()?;
        }
        _ => { ... }
    }
}
```

## Safety Matrix

| Scenario | Protocol Selected | Apply Behavior |
|----------|-------------------|----------------|
| Fresh node | Snapshot | Direct apply (no merge) |
| Initialized, >50% divergence | HashComparison | CRDT merge |
| Initialized, CLI --snapshot | **BLOCKED** | Fallback to HashComparison |
| Initialized, malicious peer | **BLOCKED** | Reject + log |

## Implementation Tasks

- [ ] Add `has_state()` check in protocol selection
- [ ] Add runtime safety check before snapshot apply
- [ ] Log all blocked snapshot attempts
- [ ] Add config option to disable override (paranoid mode)
- [ ] Metric for blocked snapshot attempts

## Logging

```
// Normal selection (fresh node)
INFO: Selected Snapshot sync for fresh node

// Safety block (initialized node)
WARN: SAFETY: Snapshot blocked for initialized node
      - using HashComparison to preserve local data
      context_id=..., configured=snapshot
```

## Acceptance Criteria

- [ ] Protocol selection never returns Snapshot for initialized nodes
- [ ] Runtime check blocks accidental snapshot apply
- [ ] Fallback to HashComparison works correctly
- [ ] Warning logged on block
- [ ] Metric incremented on block
- [ ] E2E test: initialized node rejects snapshot

## Files to Modify

- `crates/node/src/sync/manager.rs`
- `crates/node/src/sync/snapshot_sync.rs`

## POC Reference

See safety checks in `select_state_sync_strategy()` and `apply_snapshot()` in POC branch.

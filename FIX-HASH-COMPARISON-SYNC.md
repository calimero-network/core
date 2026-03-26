# BUG: HashComparison sync protocol fails to transfer data when initiator has newer state

## Status
**Severity**: Critical — breaks all fuzzy/load tests on master and CI  
**CI failure**: https://github.com/calimero-network/core/actions/runs/22663348288/job/65688985915  
**Affected since**: Current master (`04166de2`)

## What happened

The KV Store Fuzzy Load Test fails at the "Wait for seed data sync" step. After node 1 writes 10 seed values, nodes 2-4 never receive them. The root cause is **not** the NEAR sandbox crashing (the sandbox is alive the entire time and killed by merobox cleanup after the test fails).

### Observed behavior from node logs

Node 1 (has seed data) initiates sync with node 2 (missing seed data). The sync manager:

1. Detects different root hashes: `local_root=7SE6...` vs `remote_root=AfEv...`
2. Correctly selects `HashComparison` protocol  
3. **Protocol "completes successfully" but transfers zero data**
4. Reports `divergent_subtrees: []` (no divergence found)
5. Repeats every ~1 second for 30 seconds — state never converges

**Key log line** from node 1 (repeated every second for 30s):
```
Protocol selected context_id=... 
  protocol=HashComparison { root_hash: [...], divergent_subtrees: [] }
  reason=default: using hash comparison
  local_root=7SE6WmEgdeegi9w9nhA2P1dfQvETzDHnKfmd2rFtxEmC   ← has seed data
  remote_root=AfEvBVqpzVqSMyV9uYS8EQDMtMeHmwpPBsE2kUYxNDjh  ← missing seed data
  local_entities=1
  remote_entities=1
```

## Root cause analysis

### There are two interacting bugs:

### Bug 1: `entity_count` estimation is always 1

`estimate_entity_count()` in `crates/node/primitives/src/sync/state_machine.rs:125-133`:

```rust
pub fn estimate_entity_count(root_hash: [u8; 32], dag_heads_len: usize) -> u64 {
    if root_hash == [0; 32] { 0 }
    else if dag_heads_len == 0 { 1 }  // ← always 1 if no heads
    else { dag_heads_len as u64 }
}
```

With `dag_heads_count=1`, both sides always report `entity_count=1`, even though node 1 has 10+ entities. This means:
- `estimate_max_depth(1) = 1`
- `divergence` calculation is skewed
- Protocol selection doesn't have accurate info

### Bug 2: HashComparison protocol is **pull-only** — cannot push local changes to peers

In `crates/node/src/sync/hash_comparison_protocol.rs`, the initiator starts from the **remote** root hash and pulls data FROM the peer:

```rust
let mut to_compare: Vec<([u8; 32], bool)> = vec![(remote_root_hash, true)];
```

When comparing nodes, if a node exists locally but NOT remotely, it hits:

```rust
TreeCompareResult::RemoteMissing => {
    // Bidirectional sync: future work
}
```

**This is a NO-OP.** The protocol silently skips all data that the initiator has but the peer doesn't.

### Why this causes the failure:

When **node 1** (has seed data) initiates sync with **node 2** (missing seed data):
- Node 1 is the initiator. It pulls node 2's tree (old state).
- Node 2's tree is a subset of node 1's tree.
- All node 2 data exists locally → `TreeCompareResult::Equal` or `RemoteMissing`
- Result: nothing is synced. Protocol reports success with 0 transfers.

When **node 2** initiates sync with **node 1**:
- Node 2 is the initiator. It pulls node 1's tree (new state).
- Node 1's tree has extra entities node 2 doesn't have → `LocalMissing` → should recurse and merge
- **This direction SHOULD work** — but the gossipsub broadcast should also deliver deltas directly.

### Why neither direction works in practice:

The gossipsub broadcast path (primary delta delivery) is also failing or the deltas aren't being processed. This needs investigation separately, but the HashComparison protocol should be a reliable fallback.

The sync runs every ~1 second. Over 30 seconds:
- Node 1 → Node 2 direction: always no-op (pull-only, node 1 has more data)
- Node 2 → Node 1 direction: node 2 also runs sync, but something prevents it from successfully pulling the data (possibly the `entity_count=1` estimation causes wrong protocol selection, or the tree walking hits an edge case)

## How to reproduce locally

```bash
cd workflows/fuzzy-tests/kv-store
# Create a short test config
sed 's/duration_minutes: 45/duration_minutes: 2/' fuzzy-test.yml > fuzzy-test-local.yml

# Run with local merod binary
merobox bootstrap run fuzzy-test-local.yml \
  --no-docker \
  --binary-path ./target/debug/merod \
  --e2e-mode --verbose

# Test fails at "Wait for seed data sync" — nodes 2-4 never receive seed data
# Check node logs:
grep "Protocol selected" data/fuzzy-kv-node-1/logs/fuzzy-kv-node-1.log
# Shows: HashComparison with divergent_subtrees: [] despite different root hashes
```

Cleanup: `rm -rf data/ fuzzy-test-local.yml`

**Requirements**: merobox (`pip install merobox` or `brew install merobox`), VPN off (mDNS needs local network)

## Files to investigate/fix

| File | Issue |
|------|-------|
| `crates/node/src/sync/hash_comparison_protocol.rs` | **Primary fix**: `RemoteMissing` is a no-op — needs to trigger push or reverse-pull |
| `crates/node/primitives/src/sync/state_machine.rs` | `estimate_entity_count` always returns 1 when `dag_heads_len=1` |
| `crates/node/src/sync/manager.rs:1114-1117` | `divergent_subtrees` is always hardcoded to `vec![]` in the return value |
| `crates/node/src/sync/manager.rs:1006-1019` | `query_peer_dag_state` → `select_protocol` flow may need adjustment |
| `crates/node/src/handlers/state_delta.rs` | Check if gossipsub broadcast of deltas is working (primary path) |

## How to fix

### Option A: Make HashComparison bidirectional (preferred)
When the initiator detects `RemoteMissing` nodes, it should push those entities to the peer (or trigger the peer to pull them). This makes the protocol work regardless of which side initiates.

### Option B: Detect initiator-has-more and switch to push mode  
If `local_root != remote_root` and the initiator's tree is a superset, switch to a protocol where the initiator sends its extra data.

### Option C: Fix gossipsub delta broadcasting (separate issue)
The primary sync path (gossipsub broadcast of state deltas after `execute`) should deliver deltas to all subscribed peers within seconds. If this works, the periodic HashComparison is just a fallback. But gossipsub is clearly failing too — otherwise the test would pass.

### Immediate fix suggestion
The fastest path is likely:
1. Fix `estimate_entity_count` to use the actual tree/index entity count instead of guessing from DAG heads
2. In the sync manager, when HashComparison completes with 0 entities merged AND root hashes still differ, trigger a delta sync or snapshot fallback instead of silently reporting success
3. Investigate why gossipsub broadcast isn't delivering the 10 seed data deltas

## Tests to add

1. **Unit test**: HashComparison protocol where initiator has more data than peer — should still converge
2. **Unit test**: `estimate_entity_count` with realistic tree sizes  
3. **Integration test**: Write data on one node, verify sync propagation within timeout
4. **The existing fuzzy test should pass** after the fix

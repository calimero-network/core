# Issue 018: Compliance Test Suite

**Priority**: P1  
**CIP Section**: Compliance Test Plan  
**Depends On**: All core issues

## Summary

Implement the black-box compliance tests specified in the CIP to verify protocol correctness.

## Test Categories

### A. Protocol Negotiation Tests

| ID | Test | Expected |
|----|------|----------|
| N1 | Full capability match | Optimal protocol selected |
| N2 | Mixed capabilities | Graceful fallback |
| N3 | Version mismatch | Clear rejection |
| N4 | Root hash match | `None` selected, no transfer |

### B. Delta Buffering Tests

| ID | Test | Expected |
|----|------|----------|
| B1 | Buffer during snapshot | Delta replayed after sync |
| B2 | Buffer ordering | Causal order via DAG |
| B3 | Buffer overflow | No deltas dropped |

### C. CRDT Merge Tests

| ID | Test | Expected |
|----|------|----------|
| M1 | Counter merge | `final = sum(all increments)` |
| M2 | Map disjoint keys | All keys present |
| M3 | Map same key | Higher HLC wins |
| M4 | Set union | Add-wins |
| M5 | Custom type | WASM callback invoked |
| M6 | Root state | `merge_root_state()` invoked |
| M7 | Unknown type | LWW fallback |

### D. E2E Convergence Tests

| ID | Test | Expected |
|----|------|----------|
| E1 | Two-node concurrent | Root hashes match |
| E2 | Three-node | All converge |
| E3 | Fresh node | Bootstraps correctly |
| E4 | Partition heals | All converge |
| E5 | Large gap | Catches up |

### E. Security Tests

| ID | Test | Expected |
|----|------|----------|
| S1 | Tampered snapshot | Verification fails |
| S2 | Wrong root hash | Sync aborts |
| S3 | Snapshot on initialized | CRDT merge, not overwrite |

## Test Infrastructure

### Unit Tests (per module)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_n1_full_capability_match() {
        let local = SyncHandshake { ... };
        let remote = SyncHandshake { ... };
        let protocol = select_protocol(&local, &remote);
        assert_eq!(protocol, SyncProtocol::HashComparison { ... });
    }
    
    #[test]
    fn test_m1_counter_merge() {
        let local = Counter::new();
        local.increment(5);
        
        let remote = Counter::new();
        remote.increment(3);
        
        let merged = merge_counter(&local, &remote)?;
        assert_eq!(merged.value(), 8);
    }
}
```

### Integration Tests (multi-node)

```rust
#[tokio::test]
async fn test_e1_two_node_concurrent() {
    let (node_a, node_b) = setup_two_nodes().await;
    
    // Concurrent writes
    node_a.write("key_a", "value_a").await;
    node_b.write("key_b", "value_b").await;
    
    // Trigger sync
    trigger_sync(&node_a, &node_b).await;
    
    // Verify convergence
    assert_eq!(node_a.root_hash(), node_b.root_hash());
    assert_eq!(node_a.get("key_a"), Some("value_a"));
    assert_eq!(node_a.get("key_b"), Some("value_b"));
    assert_eq!(node_b.get("key_a"), Some("value_a"));
    assert_eq!(node_b.get("key_b"), Some("value_b"));
}
```

### E2E Tests (merobox)

```yaml
# workflows/sync/crdt-merge.yml
name: CRDT Merge Test
steps:
  - start_node: node_1
  - start_node: node_2
  - create_context: ctx_1
  - join_context: node_2 -> ctx_1
  - write: node_1.increment("counter", 5)
  - write: node_2.increment("counter", 3)
  - wait_for_sync: 10s
  - assert_equal: node_1.get("counter") == 8
  - assert_equal: node_2.get("counter") == 8
```

## Implementation Tasks

- [ ] Create test module structure
- [ ] Implement protocol negotiation tests (N1-N4)
- [ ] Implement delta buffering tests (B1-B3)
- [ ] Implement CRDT merge tests (M1-M7)
- [ ] Implement E2E convergence tests (E1-E5)
- [ ] Implement security tests (S1-S3)
- [ ] Add CI workflow for tests

## File Structure

```
crates/
├── storage/src/tests/
│   ├── crdt_merge.rs       # M1-M7
│   └── metadata.rs
├── node/src/sync/tests/
│   ├── negotiation.rs      # N1-N4
│   ├── buffering.rs        # B1-B3
│   └── strategies.rs
└── e2e-tests/
    └── sync/
        ├── convergence.rs  # E1-E5
        └── security.rs     # S1-S3
```

## Acceptance Criteria

- [ ] All A1-A10 compliance tests pass
- [ ] Tests run in CI
- [ ] Coverage > 80% for sync code
- [ ] E2E tests run nightly
- [ ] Failure messages are clear

## Files to Create

- `crates/storage/src/tests/*.rs`
- `crates/node/src/sync/tests/*.rs`
- `e2e-tests/sync/*.rs`
- `.github/workflows/sync-tests.yml`

## POC Reference

See existing tests in POC branch under `tests/` directories.

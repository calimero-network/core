# Sync Protocol Simulation Framework - Agent Guide

This guide is for AI agents working on the Calimero sync protocol implementation.

## Framework Location

```
crates/node/tests/
├── sync_sim/              # Simulation framework (DO NOT MODIFY without review)
│   ├── mod.rs             # Main module, prelude exports
│   ├── actions.rs         # SyncMessage, SyncActions (effects model)
│   ├── types.rs           # NodeId, MessageId, EntityId, etc.
│   ├── runtime/           # SimClock, SimRng, EventQueue
│   ├── network/           # NetworkRouter, FaultConfig, PartitionManager
│   ├── node/              # SimNode state machine
│   ├── scenarios/         # Deterministic and random scenario generators
│   ├── convergence.rs     # Convergence checking (C1-C5 properties)
│   ├── digest.rs          # StateDigest computation
│   ├── metrics.rs         # SimMetrics collection
│   └── assertions.rs      # Test assertion macros
├── sync_sim.rs            # Framework unit tests
├── sync_scenarios/        # Protocol behavior tests (ADD NEW TESTS HERE)
└── sync_compliance/       # Compliance suite for issue #1785
```

## Quick Start

```rust
use crate::sync_sim::prelude::*;

#[test]
fn test_example() {
    // Create runtime with seed for reproducibility
    let mut rt = SimRuntime::with_seed(42);
    
    // Add nodes with a scenario
    let scenario = Scenario::n_nodes_synced(3, 10); // 3 nodes, 10 shared entities
    let nodes = rt.apply_scenario(scenario);
    
    // Run until convergence or timeout
    let result = rt.run();
    
    // Assert convergence
    assert_converged!(rt);
    assert_eq!(result, StopCondition::Converged);
}
```

## Key Concepts

### SimRuntime
The orchestrator. Manages clock, event queue, nodes, and network.

```rust
let mut rt = SimRuntime::with_seed(42);           // Basic
let mut rt = SimRuntime::with_config(config);     // With custom config

rt.add_node("alice");                              // Add empty node
rt.apply_scenario(scenario);                       // Add nodes with state
rt.run();                                          // Run to completion
rt.run_until(|rt| rt.clock().now() > 1000.into()); // Run with predicate
rt.step();                                         // Single event step
```

### Scenarios

**Deterministic** (for specific test cases):
```rust
Scenario::n_nodes_synced(n, entities)      // All nodes have same state
Scenario::n_nodes_diverged(n, entities)    // Each node has unique state
Scenario::partial_overlap(n, shared, unique) // Mix of shared/unique
Scenario::force_snapshot()                 // Forces snapshot sync path
Scenario::force_none()                     // Empty nodes
```

**Random** (for property-based testing):
```rust
let config = RandomScenarioConfig::new(seed)
    .with_node_count(3, 5)
    .with_entity_count(10, 100)
    .with_divergence(0.3);
let scenario = RandomScenario::generate(&config);
```

### Fault Injection

```rust
let config = SimConfig::with_seed(42)
    .with_faults(FaultConfig::default()
        .with_loss(0.1)              // 10% message loss
        .with_latency(50, 10)        // 50ms base, 10ms jitter
        .with_reorder_window(100)    // 100ms reorder window
        .with_duplicate(0.05));      // 5% duplication

// Network partitions
rt.partition_bidirectional(&alice, &bob, None);           // Permanent
rt.partition_bidirectional(&alice, &bob, Some(1000.into())); // Temporary
rt.heal_partition(&alice, &bob);
```

### Assertions

```rust
assert_converged!(rt);                    // All nodes converged
assert_not_converged!(rt);                // Not converged
assert_entity_count!(rt, "alice", 10);    // Node has N entities
assert_has_entity!(rt, "alice", entity_id); // Node has specific entity
assert_idle!(rt, "alice");                // Node is idle (no pending work)
assert_buffer_empty!(rt, "alice");        // Delta buffer is empty
```

### Metrics

```rust
let metrics = rt.metrics();
metrics.protocol.messages_sent;
metrics.protocol.bytes_sent;
metrics.effects.messages_dropped;
metrics.convergence.converged;
metrics.convergence.time_to_converge;
```

## Writing Tests

### Where to Put Tests

| Test Type | Location | When to Use |
|-----------|----------|-------------|
| Framework tests | `sync_sim.rs` | Testing the framework itself |
| Protocol scenarios | `sync_scenarios/*.rs` | Testing sync protocol behavior |
| Compliance tests | `sync_compliance/*.rs` | Issue #1785 compliance suite |

### Test Patterns

**Basic convergence test:**
```rust
#[test]
fn test_two_nodes_converge() {
    let mut rt = SimRuntime::with_seed(42);
    let scenario = Scenario::n_nodes_diverged(2, 10);
    rt.apply_scenario(scenario);
    
    rt.run();
    
    assert_converged!(rt);
}
```

**Fault tolerance test:**
```rust
#[test]
fn test_convergence_with_packet_loss() {
    let config = SimConfig::with_seed(42)
        .with_faults(FaultConfig::default().with_loss(0.2));
    let mut rt = SimRuntime::with_config(config);
    
    // ... setup and run
    
    assert_converged!(rt);
}
```

**Partition healing test:**
```rust
#[test]
fn test_partition_healing() {
    let mut rt = SimRuntime::with_seed(42);
    let [a, b] = rt.apply_scenario(Scenario::n_nodes_diverged(2, 5));
    
    // Partition for 1000 ticks
    rt.partition_bidirectional(&a, &b, Some(1000.into()));
    
    rt.run();
    
    assert_converged!(rt); // Should converge after partition heals
}
```

**Property-based test:**
```rust
#[test]
fn test_random_scenarios_converge() {
    for seed in 0..100 {
        let config = RandomScenarioConfig::new(seed)
            .with_node_count(2, 5)
            .with_entity_count(5, 20);
        
        let mut rt = SimRuntime::with_seed(seed);
        rt.apply_scenario(RandomScenario::generate(&config));
        
        let result = rt.run();
        
        assert!(
            matches!(result, StopCondition::Converged),
            "Seed {} failed to converge", seed
        );
    }
}
```

## Invariants (DO NOT BREAK)

1. **Determinism**: Same seed MUST produce identical results
2. **No silent drops**: All message drops must be recorded in metrics
3. **Convergence properties C1-C5**: See `convergence.rs` for formal definitions
4. **Time monotonicity**: SimClock never goes backwards

## Debugging Failures

1. **Get the seed**: All random tests should log their seed
2. **Reproduce locally**: `SimRuntime::with_seed(failing_seed)`
3. **Step through**: Use `rt.step()` instead of `rt.run()`
4. **Check metrics**: `rt.metrics()` shows what happened
5. **Check convergence**: `rt.check_convergence()` returns detailed status

## Common Mistakes

- **Forgetting seed**: Always use deterministic seeds for reproducibility
- **Not checking StopCondition**: `run()` returns why it stopped
- **Ignoring metrics**: Metrics reveal silent failures
- **Hardcoding time**: Use `SimTime` and `SimDuration`, not raw numbers

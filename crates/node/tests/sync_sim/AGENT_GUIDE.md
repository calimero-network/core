# Sync Protocol Simulation Framework - Agent Guide

This guide is for AI agents working on the Calimero sync protocol implementation.

## Relationship to Production Runtime

The simulation framework replicates key aspects of the production runtime while enabling deterministic, reproducible testing without actual WASM execution or network I/O.

### What IS Replicated (Real Implementation)

| Component | Production | Simulation |
|-----------|------------|------------|
| **Merkle Tree** | `calimero-storage::Index<MainStorage>` | Same! Uses real implementation |
| **Storage Actions** | `Interface::apply_action` | Same! Real CRDT action application |
| **Hash Computation** | SHA-256 tree hashes | Same! Real hash propagation |
| **Protocol Selection** | `select_protocol()` from `calimero-node-primitives` | Same! Shared function |
| **Entity Metadata** | `Metadata { created_at, updated_at }` | Same! Real types |
| **RuntimeEnv** | Callbacks routing to RocksDB | Callbacks routing to `InMemoryDB` |

### What is NOT Replicated

| Component | Production | Simulation |
|-----------|------------|------------|
| **WASM Execution** | Full `calimero-runtime` with Wasmer | Skipped—direct state manipulation |
| **Network I/O** | libp2p gossipsub/streams | `NetworkRouter` with fault injection |
| **Time** | `SystemTime::now()` | Discrete `SimClock` |
| **Concurrency** | tokio async tasks | Sequential event processing |
| **Host Functions** | 80+ functions in `VMHostFunctions` | None—storage accessed directly |

### Why This Design?

1. **Real Merkle Tree**: HashComparison protocol depends on accurate subtree traversal.
   Using the real `calimero-storage` implementation ensures hash propagation works identically.

2. **Shared Protocol Selection**: `SimNode` implements `LocalSyncState` trait and uses
   `calimero_node_primitives::sync::protocol::select_protocol()` for consistency.

3. **Deterministic Testing**: Discrete clock and seeded RNG enable reproducible failures.

4. **Fault Injection**: `NetworkRouter` can simulate packet loss, latency, reordering,
   and partitions without actual network configuration.

### Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         PRODUCTION RUNTIME                              │
├─────────────────────────────────────────────────────────────────────────┤
│  Client Request                                                         │
│       ↓                                                                 │
│  JSON-RPC Server                                                        │
│       ↓                                                                 │
│  WASM Runtime (calimero-runtime)  ←── VMHostFunctions, VMLimits         │
│       ↓                                                                 │
│  calimero-storage (Index, Interface::apply_action)                      │
│       ↓                                                                 │
│  calimero-store (RocksDB)                                               │
│       ↓                                                                 │
│  Network (libp2p gossipsub)                                             │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                         SIMULATION RUNTIME                              │
├─────────────────────────────────────────────────────────────────────────┤
│  Test Setup (Scenario)                                                  │
│       ↓                                                                 │
│  SimRuntime (orchestrator)                                              │
│       ↓                                                                 │
│  SimNode (state machine)                                                │
│       ↓                                                                 │
│  SimStorage ─────────────────┬──────────────────────────────────────────┤
│       │                      │                                          │
│       │  calimero-storage    │  ← REAL: Index, Interface, Merkle tree   │
│       │  (same crate!)       │                                          │
│       ↓                      │                                          │
│  InMemoryDB (calimero-store) │  ← Same Store interface, memory backend  │
│       ↓                      │                                          │
│  NetworkRouter (simulated)   │  ← Fault injection, partitions           │
└─────────────────────────────────────────────────────────────────────────┘
```

### Key Code Paths

**Production**: `VMHostFunctions::persist_root_state()` → `Interface::<MainStorage>::save_raw()`

**Simulation**: `SimStorage::add_entity()` → `Interface::<MainStorage>::apply_action()`

Both use the same `calimero_storage::interface::Interface` implementation!

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
│   ├── metrics.rs         # SimMetrics collection
│   └── assertions.rs      # Test assertion macros
├── sync_sim.rs            # Framework unit tests
├── sync_scenarios/        # Protocol behavior tests (ADD NEW TESTS HERE)
└── sync_compliance/       # Compliance suite for issue #1785
```

## Quick Start

```rust
// NOTE: Minimal example - see sync_scenarios/ for real test patterns

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

## Simulation vs Production Network

This section details the differences between `NetworkRouter` (simulation) and `calimero-network` (production).

### Architecture Comparison

```text
┌─────────────────────────────────────────────────────────────────────┐
│                    PRODUCTION (calimero-network)                     │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  NetworkClient                                                       │
│       │                                                              │
│       ▼ NetworkMessage                                               │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    NetworkManager (actor)                    │    │
│  │  ┌──────────────────────────────────────────────────────┐   │    │
│  │  │               Swarm<Behaviour>                        │   │    │
│  │  │  ┌──────────┐ ┌─────────┐ ┌──────────┐ ┌──────────┐  │   │    │
│  │  │  │Gossipsub │ │   Kad   │ │  Stream  │ │Rendezvous│  │   │    │
│  │  │  │ (topics) │ │  (DHT)  │ │ (direct) │ │(discover)│  │   │    │
│  │  │  └──────────┘ └─────────┘ └──────────┘ └──────────┘  │   │    │
│  │  │  ┌──────────┐ ┌─────────┐ ┌──────────┐ ┌──────────┐  │   │    │
│  │  │  │  mDNS    │ │  Relay  │ │  DCUtR   │ │ AutoNAT  │  │   │    │
│  │  │  │ (local)  │ │  (NAT)  │ │(holepun) │ │(detect)  │  │   │    │
│  │  │  └──────────┘ └─────────┘ └──────────┘ └──────────┘  │   │    │
│  │  └──────────────────────────────────────────────────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
│       │                                                              │
│       ▼ TCP/QUIC                                                     │
│  Real Network I/O (libp2p)                                          │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│                     SIMULATION (sync_sim)                            │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Test code                                                           │
│       │                                                              │
│       ▼ SyncActions                                                  │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      SimRuntime                              │    │
│  │  ┌──────────────────────────────────────────────────────┐   │    │
│  │  │               NetworkRouter                           │   │    │
│  │  │  ┌────────────────────────────────────────────────┐  │   │    │
│  │  │  │           FaultConfig                          │  │   │    │
│  │  │  │  • message_loss_rate    • duplicate_rate       │  │   │    │
│  │  │  │  • base_latency_ms      • reorder_window_ms    │  │   │    │
│  │  │  │  • partition_probability • crash_probability   │  │   │    │
│  │  │  └────────────────────────────────────────────────┘  │   │    │
│  │  │  ┌────────────────────────────────────────────────┐  │   │    │
│  │  │  │         PartitionManager                       │  │   │    │
│  │  │  │  • Bidirectional partitions                    │  │   │    │
│  │  │  │  • Timed healing                               │  │   │    │
│  │  │  └────────────────────────────────────────────────┘  │   │    │
│  │  └──────────────────────────────────────────────────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
│       │                                                              │
│       ▼ SimEvent (in EventQueue)                                     │
│  Deterministic event processing (no real I/O)                       │
└─────────────────────────────────────────────────────────────────────┘
```

### Feature Comparison

| Feature | Production (`calimero-network`) | Simulation (`NetworkRouter`) |
|---------|--------------------------------|------------------------------|
| **Message Delivery** | Real TCP/QUIC | `SimEvent::DeliverMessage` in queue |
| **Latency** | Real network latency | Configurable `base_latency_ms + jitter` |
| **Message Loss** | Real packet loss | Configurable `message_loss_rate` (0.0-1.0) |
| **Message Reorder** | Possible in real network | Configurable `reorder_window_ms` |
| **Message Duplicate** | Rare in real network | Configurable `duplicate_rate` |
| **Network Partition** | Real disconnection | `PartitionManager` with timed healing |
| **Gossipsub Topics** | Per-context topics | ❌ Not simulated (single context) |
| **Peer Discovery** | mDNS, Kad, Rendezvous | ❌ Not simulated (nodes pre-connected) |
| **NAT Traversal** | AutoNAT, Relay, DCUtR | ❌ Not simulated |
| **Connection Setup** | TCP/QUIC handshake, TLS | ❌ Instant (no handshake) |
| **Bandwidth Limits** | Real throughput | ❌ Not simulated |
| **Encryption** | TLS/Noise per connection | Optional via `EncryptionState` |
| **Streams** | `libp2p_stream` protocol | `SimStream` (tokio channels) |

### When to Use Each

| Testing Goal | Use Simulation | Use Integration Tests |
|--------------|---------------|----------------------|
| Protocol correctness | ✅ Yes | Overkill |
| Convergence properties | ✅ Yes | Also useful |
| Fault tolerance | ✅ Yes (configurable faults) | Harder to control |
| Message ordering | ✅ Yes (reorder_window_ms) | Non-deterministic |
| Partition healing | ✅ Yes (PartitionManager) | Complex setup |
| Peer discovery | ❌ No | ✅ Yes |
| NAT traversal | ❌ No | ✅ Yes |
| Real latency behavior | ❌ No | ✅ Yes |
| Connection management | ❌ No | ✅ Yes |
| Multi-context | ❌ No | ✅ Yes |

### SimStream vs Production Stream

The `SimStream` type implements the same `SyncTransport` trait as production, allowing the **real sync protocol code** to run in simulation:

```rust
// Production (calimero-network)
pub struct Stream {
    inner: Framed<BufStream<Compat<P2pStream>>, MessageCodec>,
}

// Simulation (sync_sim)
pub struct SimStream {
    tx: Option<mpsc::Sender<Vec<u8>>>,
    rx: mpsc::Receiver<Vec<u8>>,
    buffer: VecDeque<Vec<u8>>,
    encryption: EncryptionState,
}

// Both implement:
#[async_trait]
impl SyncTransport for SimStream {  // or Stream
    async fn send(&mut self, message: &StreamMessage<'_>) -> Result<()>;
    async fn recv(&mut self) -> Result<Option<StreamMessage<'static>>>;
    async fn recv_timeout(&mut self, budget: Duration) -> Result<Option<StreamMessage<'static>>>;
    fn set_encryption(&mut self, encryption: Option<(SharedKey, Nonce)>);
    async fn close(&mut self) -> Result<()>;
}
```

This design allows `hash_comparison_sync()` and other protocol functions to run unchanged in simulation.

### Fault Injection Examples

```rust
// Light chaos (realistic network)
FaultConfig::light_chaos()
// base_latency_ms: 10, jitter: 5
// message_loss_rate: 0.01 (1%)
// reorder_window_ms: 20
// duplicate_rate: 0.01 (1%)

// Heavy chaos (stress test)
FaultConfig::heavy_chaos()
// base_latency_ms: 50, jitter: 25
// message_loss_rate: 0.1 (10%)
// reorder_window_ms: 100
// duplicate_rate: 0.05 (5%)
// partition_probability: 0.01
// crash_probability: 0.001

// Custom configuration
FaultConfig::none()
    .with_latency(100, 50)    // 100ms ± 50ms
    .with_loss(0.05)          // 5% loss
    .with_reorder(200)        // 200ms reorder window
    .with_duplicates(0.02)    // 2% duplication
    .with_partitions(0.01, 500..2000)  // 1% chance, 500-2000ms duration
```

### Key Differences Summary

1. **Determinism**: Simulation is fully deterministic (same seed = same results). Production is not.

2. **Time Model**: Simulation uses discrete `SimTime`. Production uses real `SystemTime`.

3. **Concurrency**: Simulation is sequential event processing. Production uses tokio async tasks.

4. **Discovery**: Simulation assumes all nodes can reach each other. Production must discover peers.

5. **Scope**: Simulation tests single-context sync. Production handles multiple contexts.

For protocol correctness and fault tolerance testing, use simulation. For discovery, NAT, and real network behavior, use integration tests with actual `calimero-network`.

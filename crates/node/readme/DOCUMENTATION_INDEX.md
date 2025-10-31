# Node Documentation Index

Complete guide to Calimero Node runtime documentation.

---

## Start Here

**New to Node?** → [Main README](../README.md)  
**Want to integrate?** → [Integration Guide](integration-guide.md)  
**Need examples?** → [Main README Examples](../README.md#api)

---

## For Developers

### Getting Started
1. **[Main README](../README.md)** - Overview, architecture, sync flow
2. **[Sync Protocol Guide](sync-protocol.md)** - How node sync works  
3. **[Event Handling Guide](event-handling.md)** - Event execution model

### Common Tasks
- **Add delta handling:** See [Sync Protocol - Delta Flow](sync-protocol.md#delta-flow)
- **Handle events:** See [Event Handling Guide](event-handling.md)
- **Configure sync:** See [Sync Configuration](sync-configuration.md)
- **Debug sync issues:** See [Troubleshooting](troubleshooting.md)

### Troubleshooting
- **Nodes not syncing:** See [Troubleshooting - Sync Issues](troubleshooting.md#nodes-not-syncing)
- **Events not executing:** See [Troubleshooting - Event Issues](troubleshooting.md#events-not-executing)
- **Memory growing:** See [Troubleshooting - Memory](troubleshooting.md#memory-issues)

---

## For Architects

### Understanding the System
1. **[Architecture](architecture.md)** - Internal design, components
2. **[Sync Protocol](sync-protocol.md)** - How sync works  
3. **[Event Handling](event-handling.md)** - Event execution model
4. **[Design Decisions](design-decisions.md)** - Why we built it this way

### Performance
- **[Performance Guide](performance.md)** - Latency, throughput, optimization
- **[Sync Configuration](sync-configuration.md)** - Tuning parameters

---

## By Topic

### Synchronization
- [Sync Protocol](sync-protocol.md) - Complete sync flow
- [Main README - Dual-Path](../README.md#dual-path-delta-propagation)  
- [Sync Configuration](sync-configuration.md) - Tuning guide
- [Troubleshooting - Sync](troubleshooting.md#nodes-not-syncing)

### Event Handling
- [Event Handling Guide](event-handling.md) - Complete guide
- [Main README - Event Flow](../README.md#event-handler-execution)
- [Troubleshooting - Events](troubleshooting.md#events-not-executing)

### Integration
- [Integration Guide](integration-guide.md) - How to use node in your app
- [DAG Integration](integration-guide.md#dag-integration) - How node uses DAG
- [Storage Integration](integration-guide.md#storage-integration) - CRDT actions

### Performance
- [Performance Guide](performance.md) - Complete analysis
- [Architecture - Memory](architecture.md#memory-layout)
- [Sync Configuration](sync-configuration.md) - Optimization

---

## File Map

```
crates/node/
├── README.md                      # Main entry point
├── readme/
│   ├── DOCUMENTATION_INDEX.md     # This file
│   ├── architecture.md            # Internal design
│   ├── sync-protocol.md           # How sync works
│   ├── event-handling.md          # Event execution
│   ├── sync-configuration.md      # Tuning guide
│   ├── integration-guide.md       # How to integrate
│   ├── performance.md             # Benchmarks
│   ├── design-decisions.md        # Rationale
│   └── troubleshooting.md         # Common issues
└── src/
    ├── lib.rs                     # Main types
    ├── run.rs                     # Node startup
    ├── sync/
    │   ├── manager.rs             # Sync orchestration
    │   ├── stream.rs              # P2P streams
    │   └── ...
    ├── handlers/
    │   ├── network_event.rs       # Gossipsub handling
    │   ├── state_delta.rs         # Delta processing
    │   └── ...
    └── delta_store.rs             # DAG wrapper
```

---

## Quick Links

| I want to...               | Go to...                                       |
|----------------------------|------------------------------------------------|
| Understand node basics     | [Main README](../README.md)                    |
| Learn how sync works       | [Sync Protocol](sync-protocol.md)              |
| Handle events              | [Event Handling Guide](event-handling.md)      |
| Configure sync parameters  | [Sync Configuration](sync-configuration.md)    |
| Debug sync issues          | [Troubleshooting](troubleshooting.md)          |
| Optimize performance       | [Performance Guide](performance.md)            |
| Integrate with my app      | [Integration Guide](integration-guide.md)      |
| Understand design          | [Design Decisions](design-decisions.md)        |

---

## Navigation

- **Previous**: None (root)
- **Next**: [Main README](../README.md) or [Sync Protocol](sync-protocol.md)
- **Up**: [Main Documentation](../../../README.mdx)


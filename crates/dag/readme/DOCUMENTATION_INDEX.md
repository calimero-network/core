# DAG Documentation Index

Complete guide to Calimero DAG (Directed Acyclic Graph) documentation.

---

## Start Here

**New to DAG?** → [Main README](../README.md)  
**Want to integrate?** → [API Reference](api-reference.md)  
**Need examples?** → [Main README Examples](../README.md#usage)

---

## For Developers

### Getting Started
1. **[Main README](../README.md)** - Overview, quick start, basic concepts
2. **[API Reference](api-reference.md)** - Complete API documentation  
3. **[Testing Guide](testing-guide.md)** - How to test DAG behavior

### Common Tasks
- **Add a delta:** See [API Reference - Adding Deltas](api-reference.md#adding-deltas)
- **Handle out-of-order:** See [Architecture - Buffering](architecture.md#pending-buffer)
- **Query DAG state:** See [API Reference - Queries](api-reference.md#queries)
- **Debug pending deltas:** See [Troubleshooting](troubleshooting.md#deltas-stuck-in-pending)

### Troubleshooting
- **Deltas stuck pending:** See [Troubleshooting - Pending Deltas](troubleshooting.md#deltas-stuck-in-pending)
- **Memory growing:** See [Troubleshooting - Memory Issues](troubleshooting.md#memory-growing-unbounded)
- **Apply failures:** See [Troubleshooting - Apply Errors](troubleshooting.md#delta-apply-failures)

---

## For Architects

### Understanding the System
1. **[Architecture](architecture.md)** - Internal design, data structures
2. **[Performance](performance.md)** - Complexity, memory usage, optimizations
3. **[Design Decisions](design-decisions.md)** - Why we built it this way

### Integration
- **[Node Integration](integration.md)** - How node layer uses DAG
- **[Custom Appliers](integration.md#custom-appliers)** - Implementing DeltaApplier

---

## By Topic

### Causal Ordering
- [Architecture - Causal Ordering](architecture.md#causal-ordering)
- [Main README - DAG Structure](../README.md#dag-structure)
- [Performance - Complexity Analysis](performance.md#time-complexity)

### Out-of-Order Delivery
- [Architecture - Pending Buffer](architecture.md#pending-buffer)  
- [Main README - Out-of-Order](../README.md#out-of-order-delivery)
- [Testing - Order Tests](testing-guide.md#out-of-order-tests)

### Concurrent Updates (Forks)
- [Architecture - Fork Detection](architecture.md#fork-detection)
- [Main README - Concurrent Updates](../README.md#concurrent-updates-forks)
- [Testing - Fork Tests](testing-guide.md#fork-tests)

### Performance
- [Performance Guide](performance.md) - Complete performance analysis
- [Architecture - Memory](architecture.md#memory-layout)
- [Troubleshooting - Memory](troubleshooting.md#memory-growing-unbounded)

---

## File Map

```
crates/dag/
├── README.md                      # Main entry point, quick start
├── readme/
│   ├── DOCUMENTATION_INDEX.md     # This file
│   ├── api-reference.md           # Complete API docs
│   ├── architecture.md            # Internal design  
│   ├── design-decisions.md        # Rationale
│   ├── integration.md             # How to integrate DAG
│   ├── performance.md             # Benchmarks, complexity
│   ├── testing-guide.md           # Test coverage
│   └── troubleshooting.md         # Common issues
└── src/
    ├── lib.rs                     # Main implementation
    └── tests.rs                   # Test suite
```

---

## Quick Links

| I want to...           | Go to...                                                          |
|------------------------|-------------------------------------------------------------------|
| Understand DAG basics  | [Main README](../README.md)                                       |
| Add a delta            | [API Reference](api-reference.md#adding-deltas)                   |
| Handle out-of-order    | [Architecture - Pending Buffer](architecture.md#pending-buffer)   |
| Test my integration    | [Testing Guide](testing-guide.md)                                 |
| Debug stuck deltas     | [Troubleshooting](troubleshooting.md#deltas-stuck-in-pending)     |
| Optimize performance   | [Performance Guide](performance.md)                               |
| Understand design      | [Design Decisions](design-decisions.md)                           |
| Integrate with node    | [Integration Guide](integration.md)                               |

---

## Navigation

- **Previous**: None (root)
- **Next**: [Main README](../README.md) or [API Reference](api-reference.md)
- **Up**: [Main Documentation](../../../README.mdx)

